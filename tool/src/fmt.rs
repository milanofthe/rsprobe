//! Number formats of the output: `%g` and the shortest round-trip form.

/// `x` as `printf` `%.{p}g`: `p` significant digits, trailing zeros removed,
/// exponent form below 1e-4 and from 10^p.
pub fn g(x: f64, p: usize) -> String {
    let p = p.max(1);
    if !x.is_finite() {
        return if x.is_nan() {
            "nan"
        } else if x > 0.0 {
            "inf"
        } else {
            "-inf"
        }
        .into();
    }
    let e = format!("{:.*e}", p - 1, x);
    let (m, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let strip = |s: &str| match s.contains('.') {
        true => s.trim_end_matches('0').trim_end_matches('.').to_string(),
        false => s.to_string(),
    };
    match exp < -4 || exp >= p as i32 {
        true => format!("{}e{}{:02}", strip(m), if exp < 0 { '-' } else { '+' }, exp.abs()),
        false => strip(&format!("{:.*}", (p as i32 - 1 - exp) as usize, x)),
    }
}

/// `x` in the shortest form that reads back as the same value: integral
/// values below 1e16 as integers, others with the exponent from 1e16 and
/// below 1e-4 (`1e-05`), `None` where not finite (a parameter written as
/// `null`).
pub fn num(x: f64) -> String {
    if !x.is_finite() {
        return "None".into();
    }
    if x.fract() == 0.0 && x.abs() < 1e16 {
        return format!("{}", x as i64);
    }
    let e = format!("{x:e}");
    let (m, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (sign, m) = m.strip_prefix('-').map_or(("", m), |m| ("-", m));
    let digits = m.replace('.', "");
    if !(-4..16).contains(&exp) {
        let m = match digits.len() > 1 {
            true => format!("{}.{}", &digits[..1], &digits[1..]),
            false => digits,
        };
        let s = if exp < 0 { '-' } else { '+' };
        return format!("{sign}{m}e{s}{:02}", exp.abs());
    }
    match exp < 0 {
        true => format!("{sign}0.{}{digits}", "0".repeat((-exp - 1) as usize)),
        false => {
            let k = exp as usize + 1;
            format!("{sign}{}.{}", &digits[..k], &digits[k..])
        }
    }
}

/// A path of scopes as text.
pub fn path(p: &crate::records::Path) -> String {
    name(p.iter().map(|s| s.0))
}

/// A path of names as text: joined by '/', the root as `(run)`.
pub fn name<S: AsRef<str>>(path: impl IntoIterator<Item = S>) -> String {
    let s: Vec<String> = path.into_iter().map(|x| x.as_ref().to_string()).collect();
    match s.is_empty() {
        true => "(run)".into(),
        false => s.join("/"),
    }
}

/// `%.3f` of a ratio, empty for none.
pub fn ratio(r: Option<f64>) -> String {
    r.map_or(String::new(), |r| format!("{r:.3}"))
}

/// `[lo, hi]` with three decimals, empty for none.
pub fn interval(r: Option<(f64, f64)>) -> String {
    r.map_or(String::new(), |(lo, hi)| format!("[{lo:.3}, {hi:.3}]"))
}
