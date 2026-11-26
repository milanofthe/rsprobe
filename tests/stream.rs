//! The stream as Chrome trace events, under a clock the test sets: a binary
//! of its own, since the clock is a process global.
#![cfg(feature = "on")]

use rsprobe::{add, current, info, scope, Context};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

static TIME: AtomicU64 = AtomicU64::new(0);

fn clock() -> u64 {
    TIME.load(SeqCst)
}

fn at(t: u64) {
    TIME.store(t, SeqCst);
}

fn spans(v: &Value) -> Vec<(String, u64, f64, f64)> {
    let mut x: Vec<_> = v["traceEvents"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["ph"] == "X")
        .map(|e| {
            let f = |k: &str| e[k].as_f64().unwrap();
            let name = e["name"].as_str().unwrap().to_string();
            (name, e["tid"].as_u64().unwrap(), f("ts"), f("dur"))
        })
        .collect();
    x.sort_by(|a, b| a.partial_cmp(b).unwrap());
    x
}

#[test]
fn a_nested_run_as_trace_events() {
    let ctx = Context::builder().clock(clock).stream().build();
    at(1000);
    let on = ctx.enter();
    at(2000);
    let a = scope!("a");
    at(3000);
    let b = scope!("b");
    at(5000);
    drop(b);
    add!("n", 4);
    at(5500);
    info!("msg");
    let h = current();
    std::thread::spawn(move || {
        at(6000);
        let g = h.enter();
        at(7000);
        let c = scope!("c");
        at(9000);
        drop(c);
        at(10000);
        drop(g);
    })
    .join()
    .unwrap();
    at(12000);
    drop(a);

    let open: Value = serde_json::from_str(&ctx.trace()).unwrap();
    at(20000);
    drop(on);
    let v: Value = serde_json::from_str(&ctx.trace()).unwrap();
    let s = |n: &str, t, ts, dur| (n.to_string(), t, ts, dur);
    let mut expected = vec![
        s("a", 0, 2.0, 10.0),
        s("a", 1, 6.0, 4.0),
        s("b", 0, 3.0, 2.0),
        s("c", 1, 7.0, 2.0),
    ];
    assert_eq!(spans(&open), expected);
    expected.push(s("root", 0, 1.0, 19.0));
    assert_eq!(spans(&v), expected);

    let events = v["traceEvents"].as_array().unwrap();
    let of = |ph: &str| events.iter().filter(|e| e["ph"] == ph).collect::<Vec<_>>();
    let mut threads: Vec<_> = of("M").iter().map(|e| e["tid"].as_u64()).collect();
    threads.sort();
    assert_eq!(threads, [Some(0), Some(1)]);
    let i = of("i");
    assert_eq!(
        (i.len(), &i[0]["name"], &i[0]["tid"]),
        (1, &"msg".into(), &0.into())
    );
    assert_eq!(i[0]["ts"].as_f64(), Some(5.5));
    let c: Vec<_> = of("C").into_iter().filter(|e| e["name"] == "n").collect();
    assert_eq!(
        (c.len(), &c[0]["name"], &c[0]["args"]["a 0"]),
        (1, &"n".into(), &4.into()),
        "the message adds its emit counter next to the one of n"
    );
}

#[test]
fn no_stream_no_trace() {
    let ctx = Context::new();
    let _on = ctx.enter();
    drop(scope!("a"));
    assert_eq!(ctx.trace(), "");
}
