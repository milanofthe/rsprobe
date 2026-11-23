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

const RECORD: &str = r#"{"schema":2,"params":{"n":1000,"tol":0.000000001,"inf":null,"case":"a \"quoted\"\\\u000aname\u0001"},"calibration":{"inner_ns":1000,"outer_ns":1000.244140625},"threads":2,"measures":{"calls":"exact","wall":"noisy","busy":"noisy","items":"exact"},"sites":[{"id":1,"name":"a","module":"golden","file":"tests/golden.rs","line":29,"level":"info"},{"id":3,"name":"b","module":"golden","file":"tests/golden.rs","line":32,"level":"info"},{"id":6,"name":"c","module":"golden","file":"tests/golden.rs","line":39,"level":"info"}],"nodes":[{"id":0,"parent":null,"site":null,"values":{"calls":0,"wall":24000,"busy":0}},{"id":1,"parent":0,"site":1,"values":{"calls":1,"wall":21000,"busy":6000,"items":3}},{"id":2,"parent":1,"site":3,"values":{"calls":3,"wall":9000,"busy":0,"items":3}},{"id":3,"parent":1,"site":6,"values":{"calls":1,"wall":0,"busy":4000,"items":5}}],"warnings":[{"node":2,"level":"warn","module":"golden","file":"tests/golden.rs","line":34,"time":6000,"text":"repeated","count":3},{"node":3,"level":"warn","module":"golden","file":"tests/golden.rs","line":41,"time":19000,"text":"on a worker","count":1}],"warnings_dropped":0}"#;

const TRACE: &str = r#"{"displayTimeUnit":"ns","traceEvents":[{"pid":1,"tid":0,"ph":"M","name":"thread_name","args":{"name":"thread 0"}},{"pid":1,"tid":1,"ph":"M","name":"thread_name","args":{"name":"thread 1"}},{"pid":1,"tid":1,"ph":"C","name":"items","ts":18.000,"args":{"c 1":5}},{"pid":1,"tid":1,"ph":"X","name":"c","ts":17.000,"dur":4.000,"args":{"node":3}},{"pid":1,"tid":1,"ph":"X","name":"a","ts":16.000,"dur":6.000,"args":{"node":1}},{"pid":1,"tid":0,"ph":"C","name":"items","ts":3.000,"args":{"a 0":3}},{"pid":1,"tid":0,"ph":"C","name":"items","ts":5.000,"args":{"b 0":0}},{"pid":1,"tid":0,"ph":"X","name":"b","ts":4.000,"dur":3.000,"args":{"node":2}},{"pid":1,"tid":0,"ph":"C","name":"items","ts":9.000,"args":{"b 0":1}},{"pid":1,"tid":0,"ph":"X","name":"b","ts":8.000,"dur":3.000,"args":{"node":2}},{"pid":1,"tid":0,"ph":"C","name":"items","ts":13.000,"args":{"b 0":3}},{"pid":1,"tid":0,"ph":"X","name":"b","ts":12.000,"dur":3.000,"args":{"node":2}},{"pid":1,"tid":0,"ph":"X","name":"a","ts":2.000,"dur":21.000,"args":{"node":1}},{"pid":1,"tid":0,"ph":"X","name":"root","ts":0.000,"dur":24.000,"args":{"node":0}},{"pid":1,"tid":0,"ph":"i","s":"t","name":"at the root","ts":1.000,"args":{"node":0,"level":"info"}},{"pid":1,"tid":0,"ph":"i","s":"t","name":"repeated","ts":6.000,"args":{"node":2,"level":"warn"}},{"pid":1,"tid":0,"ph":"i","s":"t","name":"repeated","ts":10.000,"args":{"node":2,"level":"warn"}},{"pid":1,"tid":0,"ph":"i","s":"t","name":"repeated","ts":14.000,"args":{"node":2,"level":"warn"}},{"pid":1,"tid":1,"ph":"i","s":"t","name":"on a worker","ts":19.000,"args":{"node":3,"level":"warn"}},{"pid":1,"tid":1,"ph":"i","s":"t","name":"1/2","ts":20.000,"args":{"node":3,"level":"info"}}]}"#;

#[test]
fn record_and_trace_are_unchanged() {
    let ctx = Context::builder().clock(clock).stream().build();
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
