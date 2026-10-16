#!/usr/bin/env bash
# Local CI: same checks as .github/workflows/ci.yml
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
