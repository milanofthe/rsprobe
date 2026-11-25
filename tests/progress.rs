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
    // A long run: nothing in its first second, then once a second and its end.
    step(200, 1);
    step(1000, 2);
    step(1500, 3);
    step(2100, 4);
    step(2600, 5);
    step(2700, 10);
    // Short runs, also long after the last event that passed: nothing.
    step(2800, 5);
    step(2900, 10);
    step(9000, 1);
    step(9050, 10);
    let mut seen = Vec::new();
    buffer.drain(|e, _| seen.push((e.time / 1_000_000, e.kind.to_string())));
    let expected = [(1500, "3/10"), (2600, "5/10"), (2700, "10/10")];
    assert_eq!(seen, expected.map(|(t, s)| (t, s.to_string())));
    // With a text and no known total: the text is formatted when the event
    // passes, and no event closes the run.
    let formatted = AtomicU64::new(0);
    let residual = |k: u64, ms: u64| {
        TIME.store(ms * 1_000_000, SeqCst);
        progress!(k, 0; "residual {}", {
            formatted.fetch_add(1, SeqCst);
            1.0 / k as f64
        });
    };
    residual(1, 10000);
    residual(2, 10100);
    residual(4, 11000);
    // The units go down: a new run, silent in its first second.
    residual(1, 13000);
    residual(2, 13900);
    residual(3, 14000);
    let mut seen = Vec::new();
    buffer.drain(|e, _| seen.push(e.kind.to_string()));
    assert_eq!(seen, ["4 residual 0.25", "3 residual 0.3333333333333333"]);
    assert_eq!(formatted.load(SeqCst), 2);
}

#[test]
fn loops_without_a_total_on_two_threads_are_two_runs() {
    use rsprobe::current;
    static CLOCK: AtomicU64 = AtomicU64::new(0);
    let buffer = Buffer::default();
    let ctx = Context::builder()
        .clock(|| CLOCK.load(SeqCst))
        .sink(buffer.clone())
        .build();
    let _on = ctx.enter();
    let h = current();
    // One call site, called in turn from this thread and from a worker
    // whose count is lower: neither restarts the other's run.
    let count = |k: u64| progress!(k, 0; "iteration");
    let worker = |k: u64| {
        std::thread::spawn(move || {
            let _g = h.enter();
            count(k)
        })
        .join()
        .unwrap()
    };
    count(50);
    CLOCK.store(900_000_000, SeqCst);
    worker(1);
    CLOCK.store(1_500_000_000, SeqCst);
    count(60);
    let mut seen = Vec::new();
    buffer.drain(|e, _| seen.push(e.kind.to_string()));
    assert_eq!(seen, ["60 iteration"]);
}

#[test]
fn threads_of_one_run_read_the_clock_in_the_order_they_pace() {
    use rsprobe::current;
    use std::cell::Cell;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};
    static CLOCK: AtomicU64 = AtomicU64::new(0);
    static READ: AtomicBool = AtomicBool::new(false);
    static OTHER_DONE: AtomicBool = AtomicBool::new(false);
    thread_local!(static HOLD: Cell<bool> = const { Cell::new(false) });
    // The clock of this thread's next read, held until the other thread's
    // event has passed or a lock keeps it out.
    fn clock() -> u64 {
        let t = CLOCK.load(SeqCst);
        if HOLD.with(|h| h.replace(false)) {
            READ.store(true, SeqCst);
            let end = Instant::now() + Duration::from_millis(200);
            while !OTHER_DONE.load(SeqCst) && Instant::now() < end {
                std::thread::yield_now();
            }
        }
        t
    }
    // One call site for both threads.
    fn report(done: u64) {
        progress!(done, 10);
    }
    let buffer = Buffer::default();
    let ctx = Context::builder()
        .clock(clock)
        .progress_interval(0)
        .sink(buffer.clone())
        .build();
    let _on = ctx.enter();
    let h = current();
    let other = std::thread::spawn(move || {
        let _g = h.enter();
        while !READ.load(SeqCst) {
            std::thread::yield_now();
        }
        CLOCK.store(2_000_000, SeqCst);
        report(2);
        OTHER_DONE.store(true, SeqCst);
    });
    CLOCK.store(1_000_000, SeqCst);
    HOLD.with(|h| h.set(true));
    report(1);
    other.join().unwrap();
    let mut seen = Vec::new();
    buffer.drain(|e, _| seen.push(e.kind.to_string()));
    seen.sort();
    assert_eq!(seen, ["1/10", "2/10"]);
}
