//! Progress events of a site pass at most once an interval, under a clock
//! the test sets: a binary of its own, since the clock is a process global.
#![cfg(feature = "on")]

use rsprobe::{progress, Buffer, Context};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

static TIME: AtomicU64 = AtomicU64::new(0);

/// Progress `done` of 10 from one call site at `ms` after the context was
/// built.
fn step(ms: u64, done: u64) {
    TIME.store(ms * 1_000_000, SeqCst);
    progress!(done, 10);
}

#[test]
fn a_site_reports_once_an_interval_and_the_end_of_a_run_it_reported() {
    let buffer = Buffer::default();
    let ctx = Context::builder()
        .clock(|| TIME.load(SeqCst))
        .sink(buffer.clone())
        .build();
    let _on = ctx.enter();
    // A long run: nothing in the first second, then once a second and its end.
    step(200, 1);
    step(1000, 2);
    step(1500, 3);
    step(2100, 4);
    step(2200, 10);
    // A short run right after it: nothing, also not its end.
    step(2300, 5);
    step(2400, 10);
    // The next run starts a second later: its first event and its end.
    step(3300, 1);
    step(3350, 10);
    let mut seen = Vec::new();
    buffer.drain(|e, _| seen.push((e.time / 1_000_000, e.kind.to_string())));
    let expected = [
        (1000, "2/10"),
        (2100, "4/10"),
        (2200, "10/10"),
        (3300, "1/10"),
        (3350, "10/10"),
    ];
    assert_eq!(seen, expected.map(|(t, s)| (t, s.to_string())));
}
