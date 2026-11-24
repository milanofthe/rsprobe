//! Measures merged by maximum (`max!`, the allocator's `peak`) and each
//! thread's values in the record.
#![cfg(feature = "on")]

use rsprobe::{add, current, max, scope, Alloc, Context, Handle, Record};
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
use std::sync::OnceLock;

#[global_allocator]
static A: Alloc = Alloc::new();

fn get(r: &Record, path: &[&str], m: &str) -> u64 {
    r.nodes[r.find(path).unwrap() as usize].get(m)
}

fn json(r: &Record) -> serde_json::Value {
    serde_json::from_str(&r.to_string()).unwrap()
}

#[test]
fn max_keeps_the_largest_value_over_calls_and_threads() {
    let ctx = Context::new();
    {
        let _on = ctx.enter();
        let _s = scope!("compile");
        for n in [3, 9, 4] {
            let _f = scope!("function");
            max!("slots", n);
            add!("functions", 1);
        }
        max!("slots", 5);
        let h = current();
        std::thread::spawn(move || {
            let _g = h.enter();
            max!("slots", 7);
        })
        .join()
        .unwrap();
    }
    let r = ctx.record();
    assert_eq!(get(&r, &["compile", "function"], "slots"), 9);
    assert_eq!(get(&r, &["compile", "function"], "functions"), 3);
    assert_eq!(get(&r, &["compile"], "slots"), 7);
    assert!(r.max.contains(&"slots"));
    let doc = json(&r);
    assert_eq!(doc["schema"], 2);
    assert_eq!(doc["measures"]["slots"], "max");
    assert_eq!(doc["measures"]["functions"], "exact");
}

/// Runs `work` on `n` threads entered through a handle to a scope `par` of
/// the calling thread, which allocates nothing while they run.
fn pool(ctx: &Context, n: u32, work: fn(u32)) {
    let _on = ctx.enter();
    let (handle, done) = (OnceLock::<Handle>::new(), AtomicU32::new(0));
    std::thread::scope(|s| {
        for k in 0..n {
            let (handle, done) = (&handle, &done);
            s.spawn(move || {
                let h = loop {
                    match handle.get() {
                        Some(h) => break *h,
                        None => std::hint::spin_loop(),
                    }
                };
                {
                    let _g = h.enter();
                    let _t = scope!("task");
                    work(k);
                }
                done.fetch_add(1, SeqCst);
            });
        }
        let _p = scope!("par");
        handle.set(current()).unwrap();
        while done.load(SeqCst) < n {
            std::hint::spin_loop();
        }
    });
}

#[test]
fn per_thread_values_show_the_load_of_each_worker() {
    let ctx = Context::new();
    pool(&ctx, 3, |k| add!("rows", 100 * (k + 1)));
    let r = ctx.record_threads();
    let task = r.find(&["par", "task"]).unwrap() as usize;
    assert_eq!(r.per_thread.len(), 4);
    assert_eq!(r.per_thread[0].index, 0);
    assert!(!r.per_thread[0].handle, "the calling thread");
    let mut rows: Vec<u64> = r.per_thread[1..]
        .iter()
        .inspect(|t| assert!(t.handle))
        .inspect(|t| assert!(t.nodes[task].get("busy") > 0))
        .map(|t| t.nodes[task].get("rows"))
        .collect();
    rows.sort();
    assert_eq!(rows, [100, 200, 300]);
    assert_eq!(r.nodes[task].get("rows"), 600);
    assert_eq!(r.per_thread[0].nodes[task].get("calls"), 0);
    let doc = json(&r);
    assert_eq!(doc["per_thread"].as_array().unwrap().len(), 4);
    assert_eq!(doc["per_thread"][1]["handle"], true);
    assert!(!ctx.record().to_string().contains("per_thread"));
    assert!(ctx.record().per_thread.is_empty());
}

#[cfg(feature = "alloc")]
#[test]
fn per_thread_allocations_separate_the_caller_from_the_pool() {
    let ctx = Context::new();
    pool(&ctx, 2, |_| drop(std::hint::black_box(vec![1u8; 64])));
    let r = ctx.record_threads();
    let (par, task) = (r.find(&["par"]).unwrap(), r.find(&["par", "task"]));
    let caller = &r.per_thread[0];
    assert!(!caller.handle);
    assert_eq!(caller.nodes[par as usize].get("allocs"), 0);
    for t in &r.per_thread[1..] {
        assert!(t.handle);
        assert_eq!(t.nodes[task.unwrap() as usize].get("allocs"), 1);
        assert_eq!(t.nodes[task.unwrap() as usize].get("peak"), 64);
    }
    assert_eq!(r.nodes[par as usize].get("allocs"), 2);
}

#[cfg(feature = "alloc")]
#[test]
fn peak_is_the_high_water_mark_above_the_entry_level() {
    let ctx = Context::new();
    let kept = std::hint::black_box(vec![1u8; 5000]);
    {
        let _on = ctx.enter();
        let _o = scope!("outer");
        let a = std::hint::black_box(vec![1u8; 1000]);
        {
            let _i = scope!("inner");
            drop(std::hint::black_box(vec![1u8; 200]));
            drop(std::hint::black_box(vec![1u8; 300]));
        }
        drop(a);
        {
            // Allocated after more was freed than the entry level held.
            let _l = scope!("lower");
            drop(kept);
            drop(std::hint::black_box(vec![1u8; 400]));
        }
        for n in [50, 700, 20] {
            let _r = scope!("repeated");
            drop(std::hint::black_box(vec![1u8; n]));
        }
    }
    let r = ctx.record();
    assert_eq!(get(&r, &["outer", "inner"], "peak"), 300);
    assert_eq!(get(&r, &["outer"], "peak"), 1300);
    assert_eq!(get(&r, &["outer", "lower"], "peak"), 0);
    assert_eq!(get(&r, &["outer", "repeated"], "peak"), 700);
    assert_eq!(get(&r, &[], "peak"), 1300);
    assert!(r.max.contains(&"peak"));
    assert_eq!(json(&r)["measures"]["peak"], "max");
}

#[test]
fn level_names_and_the_path_of_a_node() {
    use rsprobe::Level;
    assert_eq!("debug".parse(), Ok(Level::DEBUG));
    assert_eq!(" WARN ".parse(), Ok(Level::WARN));
    assert_eq!("off".parse(), Ok(Level::OFF));
    assert!("status"
        .parse::<Level>()
        .unwrap_err()
        .contains("off, error, warn"));
    for l in Level::ALL {
        assert_eq!(l.name().parse(), Ok(l));
    }
    let ctx = rsprobe::Context::builder().threshold(Level::OFF).build();
    {
        let _on = ctx.enter();
        let _s = rsprobe::scope!("not recorded");
    }
    assert_eq!(ctx.record().nodes.len(), 1);
    let ctx = rsprobe::Context::new();
    {
        let _on = ctx.enter();
        let _a = rsprobe::scope!("a");
        let _b = rsprobe::scope!("b");
    }
    let r = ctx.record();
    let b = r.find(&["a", "b"]).unwrap();
    let names: Vec<_> = r.path(b).iter().map(|s| s.name).collect();
    assert_eq!(names, ["a", "b"]);
    assert!(r.path(0).is_empty());
}
