//! The record's JSON and the trace, byte for byte, under a clock that counts
//! its reads: a binary of its own with one test, since the clock and the site
//! ids are process globals.
#![cfg(feature = "on")]

use rsprobe::{add, current, info, progress, scope, warn, Context};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

static TIME: AtomicU64 = AtomicU64::new(0);

fn clock() -> u64 {
    TIME.fetch_add(1000, SeqCst)
}

const RECORD: &str = r#"{"schema":2,"params":{"n":1000,"tol":0.000000001,"inf":null,"case":"a \"quoted\"\\\u000aname\u0001"},"calibration":{"inner_ns":1000,"outer_ns":1000.244140625},"threads":2,"measures":{"calls":"exact","wall":"noisy","busy":"noisy","emit":"noisy","items":"exact"},"sites":[{"id":2,"name":"a","module":"golden","file":"tests/golden.rs","line":33,"level":"info"},{"id":4,"name":"b","module":"golden","file":"tests/golden.rs","line":36,"level":"info"},{"id":7,"name":"c","module":"golden","file":"tests/golden.rs","line":43,"level":"info"}],"nodes":[{"id":0,"parent":null,"site":null,"values":{"calls":0,"wall":33000,"busy":0,"emit":1000}},{"id":1,"parent":0,"site":2,"values":{"calls":1,"wall":29000,"busy":9000,"items":3}},{"id":2,"parent":1,"site":4,"values":{"calls":3,"wall":12000,"busy":0,"items":3,"emit":3000}},{"id":3,"parent":1,"site":7,"values":{"calls":1,"wall":0,"busy":7000,"items":5,"emit":2000}}],"warnings":[{"node":2,"level":"warn","module":"golden","file":"tests/golden.rs","line":38,"time":9000,"text":"repeated","count":3},{"node":3,"level":"warn","module":"golden","file":"tests/golden.rs","line":45,"time":28000,"text":"on a worker","count":1}],"warnings_dropped":0}"#;

const TRACE: &str = r#"{"displayTimeUnit":"ns","traceEvents":[{"pid":1,"tid":0,"ph":"M","name":"thread_name","args":{"name":"thread 0"}},{"pid":1,"tid":1,"ph":"M","name":"thread_name","args":{"name":"thread 1"}},{"pid":1,"tid":1,"ph":"C","name":"items","ts":27.000,"args":{"c 1":5}},{"pid":1,"tid":1,"ph":"C","name":"emit","ts":30.000,"args":{"c 1":1000}},{"pid":1,"tid":1,"ph":"C","name":"emit","ts":34.000,"args":{"c 1":2000}},{"pid":1,"tid":1,"ph":"X","name":"c","ts":26.000,"dur":9.000,"args":{"node":3,"parent":1}},{"pid":1,"tid":1,"ph":"X","name":"a","ts":25.000,"dur":11.000,"args":{"node":1,"parent":0}},{"pid":1,"tid":0,"ph":"C","name":"emit","ts":4.000,"args":{"root 0":1000}},{"pid":1,"tid":0,"ph":"C","name":"items","ts":6.000,"args":{"a 0":3}},{"pid":1,"tid":0,"ph":"C","name":"items","ts":8.000,"args":{"b 0":0}},{"pid":1,"tid":0,"ph":"C","name":"emit","ts":11.000,"args":{"b 0":1000}},{"pid":1,"tid":0,"ph":"X","name":"b","ts":7.000,"dur":5.000,"args":{"node":2,"parent":1}},{"pid":1,"tid":0,"ph":"C","name":"items","ts":14.000,"args":{"b 0":1}},{"pid":1,"tid":0,"ph":"C","name":"emit","ts":17.000,"args":{"b 0":2000}},{"pid":1,"tid":0,"ph":"X","name":"b","ts":13.000,"dur":5.000,"args":{"node":2,"parent":1}},{"pid":1,"tid":0,"ph":"C","name":"items","ts":20.000,"args":{"b 0":3}},{"pid":1,"tid":0,"ph":"C","name":"emit","ts":23.000,"args":{"b 0":3000}},{"pid":1,"tid":0,"ph":"X","name":"b","ts":19.000,"dur":5.000,"args":{"node":2,"parent":1}},{"pid":1,"tid":0,"ph":"X","name":"a","ts":5.000,"dur":32.000,"args":{"node":1,"parent":0}},{"pid":1,"tid":0,"ph":"X","name":"root","ts":1.000,"dur":37.000,"args":{"node":0,"parent":null}},{"pid":1,"tid":0,"ph":"i","s":"t","name":"at the root","ts":2.000,"args":{"node":0,"level":"info"}},{"pid":1,"tid":0,"ph":"i","s":"t","name":"repeated","ts":9.000,"args":{"node":2,"level":"warn"}},{"pid":1,"tid":0,"ph":"i","s":"t","name":"repeated","ts":15.000,"args":{"node":2,"level":"warn"}},{"pid":1,"tid":0,"ph":"i","s":"t","name":"repeated","ts":21.000,"args":{"node":2,"level":"warn"}},{"pid":1,"tid":1,"ph":"i","s":"t","name":"on a worker","ts":28.000,"args":{"node":3,"level":"warn"}},{"pid":1,"tid":1,"ph":"i","s":"t","name":"1/2","ts":32.000,"args":{"node":3,"level":"info"}}]}"#;

#[test]
fn record_and_trace_are_unchanged() {
    let ctx = Context::builder()
        .clock(clock)
        .progress_interval(0)
        .stream()
        .build();
    ctx.param("n", 1000);
    ctx.param("tol", 1e-9);
    ctx.param("inf", f64::INFINITY);
    ctx.param("case", "a \"quoted\"\\\nname\u{1}");
    {
        let _on = ctx.enter();
        info!("at the root");
        let _a = scope!("a");
        add!("items", 3);
        for i in 0..3 {
            let _b = scope!("b");
            add!("items", i);
            warn!("repeated");
        }
        let h = current();
        std::thread::spawn(move || {
            let _g = h.enter();
            let _c = scope!("c");
            add!("items", 5);
            warn!("on a worker");
            progress!(1, 2);
        })
        .join()
        .unwrap();
        let _d = scope!(DEBUG, "below");
    }
    let record = ctx.record().to_string();
    let trace = ctx.trace();
    assert_eq!(record, RECORD, "\n{record}");
    assert_eq!(trace, TRACE, "\n{trace}");
}
