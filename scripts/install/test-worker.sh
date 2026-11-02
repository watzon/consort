#!/usr/bin/env bash
# Tests worker.sh against a fake release in a temporary folder. It installs a
# fake `consort` that prints its arguments, so it never pairs a machine or
# installs a service.
#
#   scripts/install/test-worker.sh
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
script="${here}/worker.sh"

root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/release" "$root/package" "$root/home"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) key="macos-arm64" ;;
  Darwin-x86_64) key="macos-x86_64" ;;
  Linux-x86_64) key="linux-x86_64" ;;
  Linux-aarch64 | Linux-arm64) key="linux-arm64" ;;
  *) echo "unsupported test machine" >&2; exit 1 ;;
esac

cat >"$root/package/consort" <<'FAKE'
#!/bin/sh
echo "fake consort $* code=${CONSORT_PAIRING_CODE:-none}"
FAKE
chmod +x "$root/package/consort"
tar -czf "$root/release/consort-${key}.tar.gz" -C "$root/package" consort
sum() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d ' ' -f 1; }
echo "$(sum "$root/release/consort-${key}.tar.gz")  consort-${key}.tar.gz" >"$root/release/SHA256SUMS"

run() {
  HOME="$root/home" SHELL=/bin/zsh CONSORT_INSTALL_DIR="$root/bin" \
    CONSORT_DOWNLOAD_BASE="file://$root/release" bash "$script" "$@"
}
check() { # check <description> <expected text> <actual text>
  case "$3" in
    *"$2"*) echo "ok   $1" ;;
    *) echo "FAIL $1: expected '$2' in: $3" >&2; exit 1 ;;
  esac
}

out="$(run --hub https://hub.example.com --code ABCD-EFGH --name box --tag gpu --tag work 2>&1)"
check "checks the checksum" "Checksum is correct." "$out"
check "puts consort in the install folder" "Installed $root/bin/consort" "$out"
check "passes the hub, the name and the tags" "worker install --hub https://hub.example.com --name box --tag gpu --tag work" "$out"
check "passes the code in the environment" "code=ABCD-EFGH" "$out"
check "adds the folder to the PATH of the shell" "export PATH=\"$root/bin:\$PATH\"" "$(cat "$root/home/.zshrc")"

out="$(run --hub https://mac.local:7601 --code ABCD-EFGH --tls-sha256 abc123 2>&1)"
check "passes the pin of the certificate" "worker install --hub https://mac.local:7601 --tls-sha256 abc123" "$out"
run --hub https://hub.example.com --code ABCD-EFGH >/dev/null 2>&1
check "adds the PATH line once" "1" "$(grep -c 'Added by the Consort installer' "$root/home/.zshrc")"

out="$(run --no-service 2>&1)"
check "installs the program alone with --no-service" "Pair this machine with" "$out"

echo "deadbeef  consort-${key}.tar.gz" >"$root/release/SHA256SUMS"
rm -rf "${root:?}/bin"
if out="$(run --hub https://hub.example.com --code X 2>&1)"; then
  echo "FAIL a wrong checksum must stop the install" >&2
  exit 1
fi
check "refuses a wrong checksum" "does not match" "$out"
[ ! -e "$root/bin/consort" ] || { echo "FAIL a refused install left a file" >&2; exit 1; }
echo "ok   leaves nothing behind after a wrong checksum"

if out="$(bash "$script" 2>&1)"; then
  echo "FAIL --hub is required" >&2
  exit 1
fi
check "needs --hub" "pass --hub" "$out"
out="$(bash "$script" --hub https://hub.example.com --dry-run 2>&1)"
check "prints a dry run" "Nothing is downloaded or changed." "$out"
echo "All installer tests passed."
