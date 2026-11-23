//! `wall` and `busy` under a clock the test sets: a binary of its own, since
//! the clock is a process global.
#![cfg(feature = "on")]

use rsprobe::{current, scope, Context};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

static TIME: AtomicU64 = AtomicU64::new(0);

fn clock() -> u64 {
    TIME.load(SeqCst)
}

fn at(t: u64) {
    TIME.store(t, SeqCst);
}

#[test]
fn wall_and_busy() {
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
    assert_eq!(node(&["par"]).get("busy"), 200);
    assert_eq!(node(&["par", "work"]).get("busy"), 50);
    assert_eq!(node(&["par", "work"]).get("wall"), 0);
    assert_eq!(node(&["par", "work"]).get("calls"), 1);
    assert_eq!(r.calibration.inner_ns, 0.0);
}
