#!/bin/sh
# The gate CI runs (.github/workflows/ci.yml), locally with rustup stable.
set -eu
cd "$(dirname "$0")/.."
# The toolchain's own binaries first: a Homebrew cargo ahead of rustup's
# lacks the wasm target.
PATH="$(dirname "$(rustup which cargo --toolchain stable)"):$PATH"
export PATH CARGO_TERM_COLOR=always
run() { echo "== $*"; "$@"; }
run cargo fmt --check
for f in "" on alloc log alloc,log; do
    run cargo clippy --all-targets --features "$f" -- -D warnings
    run cargo test --features "$f"
    # The crate in a browser: linked and importing nothing.
    run cargo build --release --example wasm_probe --target wasm32-unknown-unknown --features "$f"
    run python3 scripts/wasm_imports.py target/wasm32-unknown-unknown/release/examples/wasm_probe.wasm
done
# The tool, standard library only; its round trip builds examples/workload.rs.
run python3 -m unittest discover -s tool/tests -t tool
echo "== green"
