//! Progress events of a site pass at most once an interval, under a clock
//! the test sets: a binary of its own, since the clock is a process global.
#![cfg(feature = "on")]

use rsprobe::{progress, Buffer, Context};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

static TIME: AtomicU64 = AtomicU64::new(0);

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
    step(200, 1);
    step(1000, 2);
    step(1500, 3);
    step(2100, 4);
    step(2600, 5);
    step(2700, 10);
    step(2800, 5);
    step(2900, 10);
    step(9000, 1);
    step(9050, 10);
    let mut seen = Vec::new();
    buffer.drain(|e, _| seen.push((e.time / 1_000_000, e.kind.to_string())));
    let expected = [(1500, "3/10"), (2600, "5/10"), (2700, "10/10")];
    assert_eq!(
        seen,
        expected.map(|(t, s)| (t, s.to_string())),
        "a long run passes once a second after its first second and at its end, short runs never"
    );
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
    residual(1, 13000);
    residual(2, 13900);
    residual(3, 14000);
    let mut seen = Vec::new();
    buffer.drain(|e, _| seen.push(e.kind.to_string()));
    assert_eq!(seen, ["4 residual 0.25", "3 residual 0.3333333333333333"]);
    assert_eq!(
        formatted.load(SeqCst),
        2,
        "the text is formatted only for an event that passes"
    );
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
    assert_eq!(
        seen,
        ["60 iteration"],
        "the worker's lower count does not restart the run of this thread"
    );
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
    fn clock_held_until_the_other_thread_reported() -> u64 {
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
    fn report_from_one_site(done: u64) {
        progress!(done, 10);
    }
    let buffer = Buffer::default();
    let ctx = Context::builder()
        .clock(clock_held_until_the_other_thread_reported)
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
        report_from_one_site(2);
        OTHER_DONE.store(true, SeqCst);
    });
    CLOCK.store(1_000_000, SeqCst);
    HOLD.with(|h| h.set(true));
    report_from_one_site(1);
    other.join().unwrap();
    let mut seen = Vec::new();
    buffer.drain(|e, _| seen.push(e.kind.to_string()));
    seen.sort();
    assert_eq!(seen, ["1/10", "2/10"]);
}

#[test]
fn progress_stays_usable_after_the_clock_panicked_under_the_site_lock() {
    thread_local!(static PANICS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    fn clock_that_panics_when_told() -> u64 {
        assert!(!PANICS.get(), "the clock was told to panic");
        0
    }
    let buffer = Buffer::default();
    let ctx = Context::builder()
        .clock(clock_that_panics_when_told)
        .progress_interval(0)
        .sink(buffer.clone())
        .build();
    let _on = ctx.enter();
    PANICS.set(true);
    let panicked = std::panic::catch_unwind(|| progress!(1, 2)).is_err();
    PANICS.set(false);
    assert!(panicked);
    progress!(2, 2);
    let mut seen = Vec::new();
    buffer.drain(|e, _| seen.push(e.kind.to_string()));
    assert_eq!(seen, ["2/2"]);
}
