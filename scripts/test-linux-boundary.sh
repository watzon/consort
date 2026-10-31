#!/usr/bin/env bash
# Runs the boundary tests and the full stack tests in a Linux container, with
# the seccomp profile of deploy/session-host. The repo goes in read-only, and
# the build stays in a Docker volume. No test spends money: the optional
# check of the real CLI uses an API key that is not valid.
#
#   scripts/test-linux-boundary.sh
set -euo pipefail
cd "$(dirname "$0")/.."
image="${CONSORT_LINUX_IMAGE:-rust:1.98-slim-bookworm}"
exec docker run --rm \
  --security-opt seccomp="$PWD/deploy/session-host/seccomp.json" \
  --security-opt apparmor=unconfined \
  --security-opt systempaths=unconfined \
  -v "$PWD:/src:ro" \
  -v "$PWD/../sayso:/sayso:ro" \
  -v consort-linux-target:/target \
  -v consort-linux-cargo:/usr/local/cargo/registry \
  -e CARGO_TARGET_DIR=/target \
  -w /src \
  "$image" bash -euc '
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends bubblewrap curl git ca-certificates pkg-config libssl-dev cmake clang >/dev/null
    id -u nobody >/dev/null
    bwrap --unshare-all --die-with-parent --ro-bind /usr /usr --symlink usr/bin /bin --symlink usr/lib /lib --proc /proc --dev /dev -- /usr/bin/true
    echo "bubblewrap runs with the seccomp profile"
    cargo test --locked -p consort-harness boundary -- --nocapture
    cargo test --locked -p consort-harness egress
    cargo test --locked -p consort-worker
    cargo test --locked -p consort-hub --test full_stack
    cargo test --locked -p consort-cli boundary -- --nocapture
    # The real CLI with a key that is not valid. The provider refuses it, so
    # this spends nothing.
    curl -fsSL https://claude.ai/install.sh | bash >/dev/null
    CONSORT_BOUNDARY_CLAUDE="$HOME/.local/bin/claude" \
      cargo test --locked -p consort-cli --test boundary -- --ignored --nocapture
  '
