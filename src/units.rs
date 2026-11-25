//! Numbers for a reader: durations, counts, bytes and shares, as the lines of
//! [`Stderr`](crate::Stderr) and the tool's tables write them. Each function
//! returns a value whose `Display` writes into the formatter without
//! allocating and honours width and alignment (`{:>9}`), so a caller formats
//! the numbers inside its own messages the same way:
//! `info!("{} unknowns", units::count(n as f64))`.
//!
//! The rounding rule: a number is written with three digits, rounded to
//! three significant digits from 1 up (`971`, `4.19`, `12.3`) and to two
//! decimals below 1 (`0.80`), in the largest unit in which the rounded
//! number is at least 1, so that a scaled number stays below 1000 (999.5 us
//! is `1.00 ms`). Rounding is to the nearest, of the value as stored.
//!
//! - [`time`], from ns: `ns`, `us`, `ms`, `s`; from 60 s after rounding,
//!   rounded to tenths of a second, `m:ss.s` (59.95 s is `1:00.0`), and from
//!   one hour, rounded to seconds, `h:mm:ss`.
//! - [`count`]: below one million after rounding to an integer, an integral
//!   count exactly, with a comma between groups of three digits (`230,604`),
//!   another with three digits (`3.49`, `1,235`); from one million in `M`,
//!   `G`, `T`, `P`, `E` (`1.00 M`, `217 M`).
//! - [`bytes`]: below 1000 after rounding as a count with ` B`, from 1000 in
//!   `kB`, `MB`, `GB`, `TB`, `PB`, `EB` (powers of 1000).
//! - [`share`]: a fraction as a percentage with one decimal (`23.2 %`).
//!
//! A zero is `0` in the smallest unit (`0 ns`, `0 B`), a negative value its
//! magnitude with a `-`; only ASCII is written (`us`, not the micro sign).

use std::fmt::{self, Display, Formatter, Write};

/// A duration, written by [`time`].
#[derive(Clone, Copy, Debug)]
pub struct Time(f64);

/// A count, written by [`count`].
#[derive(Clone, Copy, Debug)]
pub struct Count(f64);

/// A size in bytes, written by [`bytes`].
#[derive(Clone, Copy, Debug)]
pub struct Bytes(f64);

/// A share, written by [`share`].
#[derive(Clone, Copy, Debug)]
pub struct Share(f64);

/// A duration of `ns` nanoseconds: `971 us`, `4.19 ms`, `1:00.3`.
pub fn time(ns: f64) -> Time {
    Time(ns)
}

/// A count of `n`: `230,604`, `217 M`.
pub fn count(n: f64) -> Count {
    Count(n)
}

/// A size of `n` bytes: `512 B`, `4.10 MB`.
pub fn bytes(n: f64) -> Bytes {
    Bytes(n)
}

/// A fraction `x` as a percentage: 0.232 is `23.2 %`.
pub fn share(x: f64) -> Share {
    Share(x)
}

/// Units of a time below a minute, in ns.
const TIME: [(f64, &str); 4] = [(1.0, "ns"), (1e3, "us"), (1e6, "ms"), (1e9, "s")];

/// Units of a count from one million.
const COUNT: [&str; 5] = ["M", "G", "T", "P", "E"];

/// Units of a size from 1000 bytes.
const BYTES: [&str; 6] = ["kB", "MB", "GB", "TB", "PB", "EB"];

/// A scaled number reaches the next unit at this value: three digits.
const NEXT: f64 = 1000.0;

/// Seconds a minute and an hour.
const MINUTE: f64 = 60.0;
const HOUR: u64 = 3600;

/// A text on the stack, so that a value is padded without allocating.
struct Buf {
    b: [u8; 48],
    n: usize,
}

impl Write for Buf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.n + s.len();
        let to = self.b.get_mut(self.n..end).ok_or(fmt::Error)?;
        to.copy_from_slice(s.as_bytes());
        self.n = end;
        Ok(())
    }
}

/// Writes through `write` and pads the text to the formatter's width; a text
/// longer than the buffer (a count beyond 10^40) is written unpadded.
fn padded(f: &mut Formatter, write: impl Fn(&mut dyn Write) -> fmt::Result) -> fmt::Result {
    let mut b = Buf { b: [0; 48], n: 0 };
    match write(&mut b) {
        // Only `str`s were written into it.
        Ok(()) => f.pad(std::str::from_utf8(&b.b[..b.n]).map_err(|_| fmt::Error)?),
        Err(_) => write(f),
    }
}

/// `x >= 0` rounded to three significant digits.
fn round3(x: f64) -> f64 {
    let mut b = Buf { b: [0; 48], n: 0 };
    match write!(b, "{x:.2e}") {
        Ok(()) => std::str::from_utf8(&b.b[..b.n])
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(x),
        Err(_) => x,
    }
}

/// `x >= 0` with three digits; from 1000 rounded to an integer, grouped.
fn digits(w: &mut dyn Write, x: f64) -> fmt::Result {
    let r = round3(x);
    if r < 1.0 {
        write!(w, "{x:.2}")
    } else if r < 10.0 {
        write!(w, "{r:.2}")
    } else if r < 100.0 {
        write!(w, "{r:.1}")
    } else {
        grouped(w, x.round())
    }
}

/// An integral `x >= 0` with a comma between groups of three digits.
fn grouped(w: &mut dyn Write, x: f64) -> fmt::Result {
    if x >= 1e21 {
        // Beyond u64 and the groups a reader counts.
        return write!(w, "{x:.0}");
    }
    let n = x as u128;
    let mut top = 1u128;
    while top * 1000 <= n {
        top *= 1000;
    }
    write!(w, "{}", n / top)?;
    while top > 1 {
        top /= 1000;
        write!(w, ",{:03}", n / top % 1000)?;
    }
    Ok(())
}

/// `x >= 0` in the first of `units` (each 1000 of the one before, the first
/// `first` of the plain number) in which it stays below 1000, or plain
/// with `plain` below `first`.
fn scaled(w: &mut dyn Write, x: f64, first: f64, units: &[&str], plain: &str) -> fmt::Result {
    if x.round() < first {
        match x.fract() == 0.0 {
            true => grouped(w, x)?,
            false => digits(w, x)?,
        }
        return w.write_str(plain);
    }
    let (mut v, mut k) = (x / first, 0);
    while round3(v) >= NEXT && k + 1 < units.len() {
        (v, k) = (v / NEXT, k + 1);
    }
    digits(w, v)?;
    write!(w, " {}", units[k])
}

/// Writes the sign of `x` and `body` of its magnitude; a value that is not
/// finite as Rust writes it.
fn signed(
    f: &mut Formatter,
    x: f64,
    body: impl Fn(&mut dyn Write, f64) -> fmt::Result,
) -> fmt::Result {
    padded(f, |w| {
        if !x.is_finite() {
            return write!(w, "{x}");
        }
        if x < 0.0 {
            w.write_str("-")?;
        }
        body(w, x.abs())
    })
}

impl Display for Time {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        signed(f, self.0, |w, ns| {
            if ns == 0.0 {
                return w.write_str("0 ns");
            }
            let mut k = 0;
            while k + 1 < TIME.len() && round3(ns / TIME[k].0) >= NEXT {
                k += 1;
            }
            let s = ns / TIME[k].0;
            if k + 1 < TIME.len() || round3(s) < MINUTE {
                digits(w, s)?;
                return write!(w, " {}", TIME[k].1);
            }
            let tenths = (s * 10.0).round() as u64;
            if tenths < 10 * HOUR {
                let (m, t) = (tenths / 600, tenths % 600);
                return write!(w, "{m}:{:02}.{}", t / 10, t % 10);
            }
            let s = s.round() as u64;
            write!(w, "{}:{:02}:{:02}", s / HOUR, s % HOUR / 60, s % 60)
        })
    }
}

impl Display for Count {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        signed(f, self.0, |w, n| scaled(w, n, 1e6, &COUNT, ""))
    }
}

impl Display for Bytes {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        signed(f, self.0, |w, n| scaled(w, n, NEXT, &BYTES, " B"))
    }
}

impl Display for Share {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        padded(f, |w| write!(w, "{:.1} %", 100.0 * self.0))
    }
}
