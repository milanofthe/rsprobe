//! The time of a sink is the measure `emit` and not part of the spans, under
//! a clock the sink advances: a binary of its own, since the clock is a
//! process global.
#![cfg(feature = "on")]

use rsprobe::{info, scope, Context, Event, Path, Sink};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

static TIME: AtomicU64 = AtomicU64::new(0);

struct SinkOf1000Ns;

impl Sink for SinkOf1000Ns {
    fn event(&self, _: &Event, _: &Path) {
        TIME.fetch_add(1000, SeqCst);
    }
}

#[test]
fn the_time_of_a_sink_is_emit_and_not_wall() {
    let ctx = Context::builder()
        .clock(|| TIME.load(SeqCst))
        .sink(SinkOf1000Ns)
        .stream()
        .build();
    {
        let _on = ctx.enter();
        let _a = scope!("a");
        TIME.fetch_add(10, SeqCst);
        info!("at a");
        {
            let _b = scope!("b");
            TIME.fetch_add(5, SeqCst);
            info!("at b");
            info!("at b again");
        }
        TIME.fetch_add(10, SeqCst);
    }
    let r = ctx.record();
    let node = |p: &[&str]| &r.nodes[r.find(p).unwrap() as usize];
    assert_eq!(node(&["a"]).get("emit"), 1000);
    assert_eq!(node(&["a", "b"]).get("emit"), 2000);
    assert_eq!(node(&["a", "b"]).get("wall"), 5);
    assert_eq!(node(&["a"]).get("wall"), 25);
    assert_eq!(r.nodes[0].get("wall"), 25);
    assert!(
        ctx.trace()
            .contains("\"name\":\"b\",\"ts\":1.010,\"dur\":2.005"),
        "the trace keeps the spans as the clock gave them"
    );
}
