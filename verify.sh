#!/usr/bin/env bash
#
# The gate. It stops at the first failure.
#
#   ./verify.sh            formatting, clippy, the tests, the browser target,
#                          and a release build of both binaries
#   ./verify.sh --quick    the same without the release build
#
# The tests are the ones inside the crates, and there are several thousand of
# them: the parser, the filters, the encryption, the font programs, the shaper,
# the rasteriser and the editor each prove their own answers.
#
# Compilation is not evidence. This says the workspace is consistent and that
# every test still answers its own known question. It says nothing about a
# document you have not shown it.

set -uo pipefail

quick=0
[[ "${1:-}" == "--quick" ]] && quick=1

cd "$(dirname "${BASH_SOURCE[0]}")"

step() {
    local name="$1"; shift
    printf '\n=== %s\n' "$name"
    if ! "$@"; then
        printf '\n=== %s FAILED\n' "$name" >&2
        exit 1
    fi
}

step "formatting" cargo fmt --all --check
step "clippy" cargo clippy --workspace --all-targets --locked -- -D warnings
step "tests" cargo test --workspace --locked

if rustup target list --installed 2>/dev/null | grep -qx wasm32-unknown-unknown; then
    step "browser target" cargo check --locked --target wasm32-unknown-unknown -p pdf-window
else
    printf '\n=== browser target: skipped\n  rustup target add wasm32-unknown-unknown\n'
fi

if [[ $quick -eq 0 ]]; then
    step "release build" cargo build --release --locked -p pdf-desktop -p pdf-cli
fi

printf '\n=== gate passed\n'
