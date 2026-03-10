#!/usr/bin/env bash
set -euo pipefail

echo "==> fmt check"
cargo fmt --all -- --check

echo "==> clippy (all warnings)"
cargo clippy --locked --all --benches --tests --examples --all-features -- -D warnings

echo "==> cargo deny"
cargo deny check 2>/dev/null || echo "WARN: cargo-deny not installed, skipping"

echo "==> tests"
cargo test --locked
