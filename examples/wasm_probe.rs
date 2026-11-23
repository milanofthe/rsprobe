//! The whole API on one path, built for `wasm32-unknown-unknown` by CI, which
//! then checks that the module imports nothing.
//!
//!     cargo build --release --example wasm_probe --target wasm32-unknown-unknown --features on
//!     python3 scripts/wasm_imports.py target/wasm32-unknown-unknown/release/examples/wasm_probe.wasm

use rsprobe::{add, current, scope, Context, Level};
use std::hint::black_box;

#[global_allocator]
static A: rsprobe::Alloc = rsprobe::Alloc::new();

fn main() {
    let ctx = Context::with(Level::DEBUG, rsprobe::now);
    ctx.param("n", black_box(64));
    {
        let _on = ctx.enter();
        let _s = scope!("solve");
        add!(DEBUG, "iterations", black_box(3));
        let _g = current().enter();
        let _t = scope!(TRACE, "step");
    }
    black_box(ctx.record().to_string());
}
