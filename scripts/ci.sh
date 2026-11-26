#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
rustup_toolchain_bin="$(dirname "$(rustup which cargo --toolchain stable)")"
PATH="$rustup_toolchain_bin:$PATH"
export PATH CARGO_TERM_COLOR=always
run() { echo "== $*"; "$@"; }
run cargo fmt --all --check
target_dir=${CARGO_TARGET_DIR:-target}
for f in "" on alloc log alloc,log; do
    run cargo clippy --all-targets --features "$f" -- -D warnings
    run cargo test --features "$f"
    run cargo build --release --example wasm_probe --target wasm32-unknown-unknown --features "$f"
    run cargo run --quiet -p rsprobe-tool --example wasm_imports -- "$target_dir/wasm32-unknown-unknown/release/examples/wasm_probe.wasm"
done
run cargo clippy -p rsprobe-tool --all-targets -- -D warnings
run cargo test -p rsprobe-tool
echo "== green"
