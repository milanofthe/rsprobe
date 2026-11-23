//! Nanoseconds per scope, per `add!`, per message and per allocation and free
//! with and without the counting allocator, the median of rounds, next to the
//! calibration constants of a context.
//!
//!     cargo run --release --example bench                 # feature off
//!     cargo run --release --example bench --features on
//!     cargo run --release --example bench --features alloc

use rsprobe::{add, current, debug, info, scope, Alloc, Context, Level};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::time::Instant;

/// Rounds per case; the median is printed.
const ROUNDS: usize = 21;
/// Iterations per round: about 1 to 5 ms per round at the measured costs.
const N: u32 = 100_000;

#[global_allocator]
static A: Alloc = Alloc::new();

/// Median over rounds of the time per call of `f`, in ns.
fn time(per: u32, f: impl Fn(u32)) -> f64 {
    let mut v: Vec<f64> = (0..ROUNDS)
        .map(|_| {
            let t = Instant::now();
            for i in 0..N {
                f(black_box(i));
            }
            t.elapsed().as_nanos() as f64 / (N * per) as f64
        })
        .collect();
    v.sort_by(f64::total_cmp);
    v[ROUNDS / 2]
}

fn row(case: &str, ns: f64) {
    println!("{case:<40} {ns:>8.2}");
}

fn flat(_: u32) {
    let _s = scope!("flat");
}

fn nested(_: u32) {
    let _a = scope!("n1");
    let _b = scope!("n2");
    let _c = scope!("n3");
    let _d = scope!("n4");
}

/// An allocation and a free of `size` bytes through `a`.
fn alloc_free(a: &impl GlobalAlloc, size: usize) {
    let l = Layout::from_size_align(size, 8).unwrap();
    // SAFETY: a non-zero size; the block is freed with its layout.
    unsafe { a.dealloc(black_box(a.alloc(l)), l) }
}

fn main() {
    println!("{:<40} {:>8}", "case", "ns");
    // Warm-up: the first case timed is otherwise 1 to 3 ns slower.
    time(1, |_| alloc_free(&System, 16));
    for size in [16, 65536] {
        row(
            &format!("alloc+free {size} B, System"),
            time(1, |_| alloc_free(&System, size)),
        );
        row(
            &format!("alloc+free {size} B, Alloc"),
            time(1, |_| alloc_free(&A, size)),
        );
    }
    if cfg!(not(feature = "on")) {
        row("scope, feature off", time(1, flat));
        row("add!, feature off", time(1, |i| add!("n", i)));
        return;
    }
    row("scope, no context", time(1, flat));
    let nothing = current();
    row(
        "Handle::enter, no context",
        time(1, |_| drop(nothing.enter())),
    );
    let ctx = Context::new();
    let _on = ctx.enter();
    let h = current();
    row(
        "Handle::enter, context on another thread",
        std::thread::scope(|s| s.spawn(|| time(1, |_| drop(h.enter()))).join().unwrap()),
    );
    row(
        "scope, below the threshold",
        time(1, |_| {
            let _s = scope!(DEBUG, "fine");
        }),
    );
    {
        let _other = Context::builder().threshold(Level::TRACE).build();
        row(
            "scope, below this context, other at TRACE",
            time(1, |_| {
                let _s = scope!(DEBUG, "fine");
            }),
        );
    }
    row("scope, on, flat", time(1, flat));
    row("scope, on, nested 4 deep", time(4, nested));
    row("add!, on", time(1, |i| add!("n", i)));
    row("message, below the threshold", time(1, |i| debug!("{i}")));
    row("message, on, no sink", time(1, |i| info!("{i}")));
    {
        let streamed = Context::builder().stream().build();
        let _on = streamed.enter();
        row("scope, on, flat, stream", time(1, flat));
        row("add!, on, stream", time(1, |i| add!("n", i)));
    }
    let c = ctx.record().calibration;
    row("calibration inner", c.inner_ns);
    row("calibration outer", c.outer_ns);
    row("calibration inner + outer", c.inner_ns + c.outer_ns);
}
