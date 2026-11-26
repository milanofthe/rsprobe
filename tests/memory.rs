//! The allocator measures `bytes`, `allocs` and `freed` with `Alloc` installed.
#![cfg(feature = "alloc")]

mod common;

use common::get;
use rsprobe::{current, scope, Alloc, Context};
use std::hint::black_box;

#[global_allocator]
static A: Alloc = Alloc::new();

#[test]
fn scopes_count_their_allocations_inclusively() {
    let ctx = Context::new();
    {
        let _on = ctx.enter();
        let _a = scope!("outer");
        let big = black_box(Box::new([1u8; 1000]));
        {
            let _b = scope!("inner");
            for _ in 0..3 {
                black_box(Box::new([1u8; 32]));
            }
            let _c = scope!("idle");
        }
        drop(big);
    }
    let r = ctx.record();
    for (path, bytes, allocs) in [
        (&["outer", "inner", "idle"][..], 0, 0),
        (&["outer", "inner"], 96, 3),
        (&["outer"], 1096, 4),
        (&[], 1096, 4),
    ] {
        assert_eq!(get(&r, path, "bytes"), bytes, "{path:?}");
        assert_eq!(get(&r, path, "freed"), bytes, "{path:?}");
        assert_eq!(get(&r, path, "allocs"), allocs, "{path:?}");
    }
    let json = r.to_string();
    assert!(json.contains("\"bytes\":\"exact\""));
    assert!(json.contains("\"bytes\":0,\"allocs\":0,\"freed\":0"));
}

#[test]
fn reallocation_counts_as_allocation_of_the_new_size_and_free_of_the_old() {
    let ctx = Context::new();
    let mut v: Vec<u8> = black_box(Vec::with_capacity(16));
    {
        let _on = ctx.enter();
        let _s = scope!("grow");
        v.reserve_exact(64);
        black_box(&v);
    }
    let r = ctx.record();
    assert_eq!(get(&r, &["grow"], "bytes"), 64);
    assert_eq!(get(&r, &["grow"], "allocs"), 1);
    assert_eq!(get(&r, &["grow"], "freed"), 16);
}

#[test]
fn a_handle_counts_the_worker_under_its_node() {
    let ctx = Context::new();
    {
        let _on = ctx.enter();
        let _p = scope!("par");
        let h = current();
        std::thread::spawn(move || {
            let _g = h.enter();
            let _t = scope!("task");
            black_box(Box::new([1u8; 200]));
        })
        .join()
        .unwrap();
    }
    let r = ctx.record();
    assert_eq!(get(&r, &["par", "task"], "bytes"), 200);
    assert_eq!(get(&r, &["par", "task"], "allocs"), 1);
    assert!(get(&r, &["par"], "bytes") >= 200);
}

#[test]
fn no_allocation_omits_the_measures() {
    let ctx = Context::new();
    {
        let _on = ctx.enter();
        let _s = scope!("idle");
    }
    let r = ctx.record();
    assert!(!r.measures.iter().any(|m| m.0 == "bytes"));
    assert_eq!(r.nodes[1].values.len(), 3);
}
