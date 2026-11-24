//! The whole API on one path, built for `wasm32-unknown-unknown` by CI, which
//! then checks that the module imports nothing.
//!
//!     cargo build --release --example wasm_probe --target wasm32-unknown-unknown --features on
//!     cargo run -p rsprobe-tool --example wasm_imports -- target/wasm32-unknown-unknown/release/examples/wasm_probe.wasm

use rsprobe::{add, current, info, progress, scope, warn, Buffer, Context, Level, Stderr};
use std::hint::black_box;

#[global_allocator]
static A: rsprobe::Alloc = rsprobe::Alloc::new();

fn main() {
    let buffer = Buffer::default();
    let b = Context::builder()
        .threshold(Level::DEBUG)
        .filter("wasm_probe::quiet", Level::WARN)
        .clock(rsprobe::now)
        .sink(Stderr::default())
        .sink(buffer.clone())
        .stream();
    #[cfg(feature = "log")]
    let b = b.sink(rsprobe::LogSink);
    let ctx = b.build();
    ctx.param("n", black_box(64));
    {
        let _on = ctx.enter();
        let _s = scope!("solve");
        add!(DEBUG, "iterations", black_box(3));
        info!("n = {}", black_box(64));
        warn!("residual {:.1e}", black_box(1e-3));
        progress!(black_box(1), 2);
        let _g = current().enter();
        let _t = scope!(TRACE, "step");
    }
    buffer.drain(|e, path| {
        black_box((e, path));
    });
    black_box(ctx.record().to_string());
    black_box(ctx.trace());
}
