#!/bin/sh
# The gate CI runs (.github/workflows/ci.yml), locally with rustup stable.
set -eu
cd "$(dirname "$0")/.."
# The toolchain's own binaries first: a Homebrew cargo ahead of rustup's
# lacks the wasm target.
PATH="$(dirname "$(rustup which cargo --toolchain stable)"):$PATH"
export PATH CARGO_TERM_COLOR=always
run() { echo "== $*"; "$@"; }
run cargo fmt --all --check
# The target directory, as cargo resolves it from the environment.
T=${CARGO_TARGET_DIR:-target}
for f in "" on alloc log alloc,log; do
    run cargo clippy --all-targets --features "$f" -- -D warnings
    run cargo test --features "$f"
    # The crate in a browser: linked and importing nothing.
    run cargo build --release --example wasm_probe --target wasm32-unknown-unknown --features "$f"
    run cargo run --quiet -p rsprobe-tool --example wasm_imports -- "$T/wasm32-unknown-unknown/release/examples/wasm_probe.wasm"
done
# The tool (tool/); its round trip builds examples/workload.rs.
run cargo clippy -p rsprobe-tool --all-targets -- -D warnings
run cargo test -p rsprobe-tool
echo "== green"
