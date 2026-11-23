//! No allocation on the measured path once a node exists on the thread.
#![cfg(feature = "on")]

use rsprobe::{add, current, scope, Context};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.with(|a| a.set(a.get() + 1));
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.with(|a| a.set(a.get() + 1));
        System.realloc(p, l, n)
    }
}

#[global_allocator]
static A: Counting = Counting;

fn allocs() -> u64 {
    ALLOCS.with(|a| a.get())
}

fn work() {
    let _a = scope!("a");
    for i in 0..10 {
        let _b = scope!("b");
        add!("iterations", 1);
        add!("nnz", i);
        let _c = scope!(DEBUG, "below");
    }
}

/// Allocations of the first and the second of two passes of `pass`.
fn passes(pass: impl Fn()) -> (u64, u64) {
    let a = allocs();
    pass();
    let b = allocs();
    pass();
    (b - a, allocs() - b)
}

#[test]
fn measured_path_does_not_allocate() {
    let ctx = Context::new();
    let _on = ctx.enter();
    let _p = scope!("par");
    assert_eq!(passes(work).1, 0, "entering thread");
    let h = current();
    assert_eq!(
        passes(|| {
            let _g = h.enter();
            work();
        })
        .1,
        0,
        "handle on the entering thread"
    );
    std::thread::spawn(move || {
        let (first, second) = passes(|| {
            let _g = h.enter();
            work();
        });
        assert!(first > 0, "the first pass creates this thread's cells");
        assert_eq!(second, 0, "handle on another thread");
    })
    .join()
    .unwrap();
}
