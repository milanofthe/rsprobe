//! A small nested workload over a problem size and a thread count. It writes
//! its record to the file named by `RSPROBE_RECORD`, else to stdout; the tool's
//! round-trip tests read it.
//!
//!     cargo run --example workload --features on -- N THREADS

use rsprobe::{add, current, scope, Context};
use std::hint::black_box;

/// Problem size when not given.
const N: usize = 2000;
/// Worker threads when not given.
const THREADS: usize = 2;
/// Iterations of the solve.
const ITERATIONS: usize = 5;

/// Order n.
fn assemble(n: usize) -> Vec<f64> {
    let _s = scope!("assemble");
    add!("nonzeros", n);
    (0..n).map(|i| ((i * 7919) % n) as f64).collect()
}

/// Order n log n.
fn sort(v: &mut [f64]) {
    let _s = scope!("sort");
    v.sort_by(f64::total_cmp);
}

/// Order n per iteration, each iteration a scope.
fn solve(v: &[f64]) -> f64 {
    let _s = scope!("solve");
    let mut x = 0.0;
    for _ in 0..ITERATIONS {
        let _i = scope!("iteration");
        add!("iterations", 1);
        x += v.iter().sum::<f64>();
    }
    x
}

/// Order n^2, split over worker threads that enter the section's node.
fn pairs(v: &[f64], threads: usize) -> f64 {
    let _s = scope!("pairs");
    let h = current();
    let chunk = v.len().div_ceil(threads);
    std::thread::scope(|s| {
        let tasks: Vec<_> = v
            .chunks(chunk)
            .map(|c| {
                s.spawn(move || {
                    let _g = h.enter();
                    let _t = scope!("task");
                    add!("differences", c.len() * v.len());
                    c.iter()
                        .map(|a| v.iter().map(|b| (a - b).abs()).sum::<f64>())
                        .sum::<f64>()
                })
            })
            .collect();
        tasks.into_iter().map(|t| t.join().unwrap()).sum()
    })
}

fn main() {
    let mut args = std::env::args()
        .skip(1)
        .map(|a| a.parse::<usize>().expect("arguments: N THREADS"));
    let n = args.next().unwrap_or(N);
    let threads = args.next().unwrap_or(THREADS);
    let ctx = Context::new();
    ctx.param("n", n);
    ctx.param("threads", threads);
    {
        let _on = ctx.enter();
        let mut v = assemble(n);
        sort(&mut v);
        black_box(solve(&v));
        black_box(pairs(&v, threads));
    }
    let record = ctx.record().to_string();
    match std::env::var_os("RSPROBE_RECORD") {
        Some(path) => std::fs::write(path, record).expect("writing the record"),
        None => println!("{record}"),
    }
}
