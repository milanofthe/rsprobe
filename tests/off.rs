//! Without the feature `on` the macros expand to `()`.
#![cfg(not(feature = "on"))]

use rsprobe::{add, scope, Context};

fn evaluated() -> u64 {
    panic!("the value of add! is evaluated")
}

#[test]
fn macros_expand_to_nothing() {
    let ctx = Context::new();
    let _on = ctx.enter();
    let s = scope!("a");
    let t = scope!(DEBUG, "b");
    assert_eq!(std::mem::size_of_val(&s) + std::mem::size_of_val(&t), 0);
    add!("n", evaluated());
    add!(DEBUG, "n", evaluated());
    let r = ctx.record();
    assert_eq!(r.nodes.len(), 1);
    assert_eq!(r.calibration.inner_ns, 0.0);
}
