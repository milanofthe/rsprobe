//! A worker holds the context it entered through a handle; `active!`.
#![cfg(feature = "on")]

use rsprobe::{active, current, scope, Context, Event, Handle, Level, Path, Sink};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{mpsc, Arc};

struct Freed(Arc<AtomicBool>);

impl Sink for Freed {
    fn event(&self, _: &Event, _: &Path) {}
}

impl Drop for Freed {
    fn drop(&mut self) {
        let _borrows_the_thread_state = current();
        self.0.store(true, SeqCst);
    }
}

fn freed_around_the_worker(then: impl FnOnce(Handle, Handle) + Send + 'static) -> (bool, bool) {
    let freed = Arc::new(AtomicBool::new(false));
    let ctx = Context::builder().sink(Freed(freed.clone())).build();
    let other = Context::new();
    let (h, o) = {
        let _on = ctx.enter();
        let _p = scope!("par");
        let h = current();
        let _other = other.enter();
        (h, current())
    };
    let (go, wait) = mpsc::channel();
    let (done, entered) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        for _ in 0..3 {
            let _g = h.enter();
            let _t = scope!("task");
        }
        done.send(()).unwrap();
        wait.recv().unwrap();
        then(h, o);
        done.send(()).unwrap();
        wait.recv().unwrap();
    });
    entered.recv().unwrap();
    let r = ctx.record();
    let task = r.find(&["par", "task"]).unwrap() as usize;
    assert_eq!(r.nodes[task].get("calls"), 3);
    drop(ctx);
    let before = freed.load(SeqCst);
    go.send(()).unwrap();
    entered.recv().unwrap();
    let after = freed.load(SeqCst);
    go.send(()).unwrap();
    worker.join().unwrap();
    (before, after)
}

#[test]
fn a_worker_lets_go_when_it_enters_another_context() {
    let (before, after) = freed_around_the_worker(|_, o| drop(o.enter()));
    assert!(!before && after);
}

#[test]
fn a_worker_lets_go_when_it_enters_a_dropped_context() {
    let (before, after) = freed_around_the_worker(|h, _| {
        let _g = h.enter();
        assert!(!active!());
    });
    assert!(!before && after);
}

#[test]
fn a_worker_lets_go_when_its_guard_drops_after_the_context() {
    let freed = Arc::new(AtomicBool::new(false));
    let ctx = Context::builder().sink(Freed(freed.clone())).build();
    let _keeps_sites_active = Context::new();
    let h = {
        let _on = ctx.enter();
        current()
    };
    let (go, wait) = mpsc::channel();
    let (done, entered) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let g = h.enter();
        done.send(()).unwrap();
        wait.recv().unwrap();
        drop(g);
    });
    entered.recv().unwrap();
    drop(ctx);
    assert!(!freed.load(SeqCst));
    go.send(()).unwrap();
    worker.join().unwrap();
    assert!(freed.load(SeqCst));
}

#[test]
fn a_thread_that_ends_lets_go() {
    let freed = Arc::new(AtomicBool::new(false));
    let ctx = Context::builder().sink(Freed(freed.clone())).build();
    let h = {
        let _on = ctx.enter();
        current()
    };
    std::thread::spawn(move || drop(h.enter())).join().unwrap();
    drop(ctx);
    assert!(freed.load(SeqCst));
}

#[test]
fn active_follows_the_threshold_and_the_filters() {
    assert!(!active!());
    let ctx = Context::builder()
        .filter(module_path!(), Level::DEBUG)
        .build();
    let _on = ctx.enter();
    assert!(active!() && active!(DEBUG) && !active!(TRACE));
}
