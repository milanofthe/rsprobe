//! The built-in times and counts under a clock the test sets: a binary of
//! its own, since the clock is a process global; the tests run one at a
//! time through a lock.
#![cfg(feature = "on")]

use rsprobe::{current, scope, Context};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::Mutex;

static TIME: AtomicU64 = AtomicU64::new(0);
static SERIAL: Mutex<()> = Mutex::new(());

fn clock() -> u64 {
    TIME.load(SeqCst)
}

fn at(t: u64) {
    TIME.store(t, SeqCst);
}

#[test]
fn wall_busy_and_pool() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let ctx = Context::builder().clock(clock).build();
    at(100);
    let on = ctx.enter();
    at(200);
    let par = scope!("par");
    let h = current();
    std::thread::spawn(move || {
        at(300);
        let g = h.enter();
        at(400);
        let w = scope!("work");
        at(450);
        drop(w);
        at(500);
        drop(g);
    })
    .join()
    .unwrap();
    at(600);
    drop(par);
    at(1000);
    drop(on);

    let r = ctx.record();
    let node = |p: &[&str]| &r.nodes[r.find(p).unwrap() as usize];
    assert_eq!(r.nodes[0].get("wall"), 900);
    assert_eq!(node(&["par"]).get("wall"), 400);
    // the section's scope ran on the calling thread only
    assert_eq!(node(&["par"]).get("busy"), 0);
    assert_eq!(node(&["par"]).get("worker_calls"), 0);
    assert_eq!(node(&["par"]).get("pool"), 200);
    assert_eq!(node(&["par"]).get("tasks"), 1);
    let work = node(&["par", "work"]);
    assert_eq!([work.get("wall"), work.get("busy")], [0, 50]);
    assert_eq!([work.get("calls"), work.get("worker_calls")], [1, 1]);
    assert_eq!([work.get("pool"), work.get("tasks")], [0, 0]);
    assert_eq!(r.calibration.inner_ns, 0.0);
    // zero split measures are not written
    assert!(!work.values.iter().any(|v| v.0 == "pool" || v.0 == "tasks"));
}

/// A scope that runs once on the calling thread and once on a worker, and a
/// section whose tasks run once on a worker and once on the calling thread.
#[test]
fn a_scope_on_both_kinds_of_thread() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let ctx = Context::builder().clock(clock).build();
    at(0);
    let on = ctx.enter();
    at(100);
    let sec = scope!("sec");
    let h = current();
    let task = move |t: u64| {
        at(t);
        let g = h.enter();
        at(t + 10);
        let w = scope!("work");
        at(t + 40);
        drop(w);
        at(t + 50);
        drop(g);
    };
    std::thread::spawn(move || task(200)).join().unwrap();
    // the calling thread runs a task itself: the node moves, no task counted
    task(300);
    at(400);
    drop(sec);
    at(500);
    drop(on);

    let r = ctx.record_threads();
    let node = |p: &[&str]| &r.nodes[r.find(p).unwrap() as usize];
    let (sec, work) = (node(&["sec"]), node(&["sec", "work"]));
    assert_eq!(
        [sec.get("calls"), sec.get("wall"), sec.get("busy")],
        [1, 300, 0]
    );
    assert_eq!([sec.get("pool"), sec.get("tasks")], [50, 1]);
    assert_eq!([work.get("calls"), work.get("worker_calls")], [2, 1]);
    assert_eq!([work.get("wall"), work.get("busy")], [30, 30]);
    assert_eq!(r.nodes[0].get("wall"), 500);
    // per thread the same split
    let w = r.find(&["sec", "work"]).unwrap() as usize;
    let of = |handle: bool| {
        &r.per_thread
            .iter()
            .find(|t| t.handle == handle)
            .unwrap()
            .nodes[w]
    };
    assert_eq!(
        [of(false).get("calls"), of(false).get("worker_calls")],
        [1, 0]
    );
    assert_eq!(
        [of(true).get("calls"), of(true).get("worker_calls")],
        [1, 1]
    );
    assert_eq!([of(true).get("busy"), of(false).get("wall")], [30, 30]);
}
