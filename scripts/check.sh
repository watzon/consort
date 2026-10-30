#!/usr/bin/env bash
# The checks that must pass before a change is done: format, lint and tests.
#
#   scripts/check.sh                       the whole workspace
#   scripts/check.sh consort-hub consort-proto   only these crates
#
# Tests run with cargo-nextest when it is installed: it runs the test
# binaries in parallel and each test in its own process. Without it, the
# script falls back to `cargo test`. Doc tests run with `cargo test --doc`,
# because nextest does not run them.
set -euo pipefail
cd "$(dirname "$0")/.."

scope=(--workspace)
if [ "$#" -gt 0 ]; then
  scope=()
  for crate in "$@"; do scope+=(-p "$crate"); done
fi

step() { printf '\n== %s\n' "$1"; }
start=$SECONDS

step "Format"
cargo fmt --all --check

step "Lint"
cargo clippy "${scope[@]}" --all-targets --quiet -- -D warnings

step "Tests"
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run "${scope[@]}" --no-fail-fast --status-level fail --final-status-level fail
else
  cargo test "${scope[@]}" --no-fail-fast --quiet
fi

step "Doc tests"
cargo test "${scope[@]}" --doc --quiet

printf '\nAll checks passed in %ss.\n' "$((SECONDS - start))"
