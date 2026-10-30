#!/usr/bin/env bash
# Development run: build the debug binaries, start a dev hub with its worker
# (`consort dev`), start the desktop app, and show the hub's log. Quitting the
# app stops the hub.
#
#   scripts/dev.sh [app flags]      for example: scripts/dev.sh --dark --theme velvet
#
# The hub, its worker and the app's pairing live in CONSORT_DEV_DIR (default
# ~/.consort-dev), apart from the normal Consort folders. A dev run never
# changes the pairing of an app that you use with a real hub. The folder is
# outside the repo because task workspaces go in it, and a harness that works
# there must not find this repo's AGENTS.md or git history. Delete the folder
# to start again with a new hub.
#
# The worker's Consort home (its account profile folders and managed tools)
# is CONSORT_DEV_DIR/hub/home (`consort dev` uses <data>/home), so a dev run
# never uses the logins and tools in ~/.consort. Set CONSORT_HOME to use
# another folder.
#
# The first run prints the hub URL and a setup code. Type them in the app
# once. Later runs connect without them.
#
# CONSORT_DEV_LISTEN sets the hub address (default 127.0.0.1:7600). Set it to
# this machine's LAN address to pair a worker on another machine. RUST_LOG
# passes through. With --mock, only the app starts, with no hub. Press Ctrl+C
# to stop the app and the hub.
#
# On macOS the app runs from a small bundle in the build folder, because macOS
# takes the Dock icon, the menu name and notifications from a bundle. It has
# its own bundle ID, so it stays apart from an installed Consort.
set -euo pipefail
cd "$(dirname "$0")/.."

case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*)
    echo "error: scripts/dev.sh runs on macOS and Linux only." >&2
    exit 1
    ;;
esac

dev_dir="${CONSORT_DEV_DIR:-$HOME/.consort-dev}"
listen="${CONSORT_DEV_LISTEN:-127.0.0.1:7600}"

echo "==> building"
cargo build -p consort-cli -p consort-desktop
bin="$(cd "${CARGO_TARGET_DIR:-target}" && pwd)/debug"

app_bin="$bin/consort-desktop"
if [[ "$(uname -s)" == Darwin ]]; then
  # The folder name starts with "consort", so stop_earlier knows the app.
  env -u APPLE_SIGNING_IDENTITY scripts/bundle-macos.sh \
    --binary "$bin/consort-desktop" --no-archive \
    --out "$bin/consort-dev-bundle" --bundle-id dev.consort.desktop.dev > /dev/null
  app_bin="$bin/consort-dev-bundle/Consort.app/Contents/MacOS/Consort"
fi

for arg in "$@"; do
  if [[ "$arg" == --mock ]]; then
    exec "$app_bin" "$@"
  fi
done

mkdir -p "$dev_dir"
chmod 700 "$dev_dir"
pid_file="$dev_dir/dev.pids"
log="$dev_dir/hub.log"

# Stops a hub or an app that an earlier run started, if it still runs.
stop_earlier() {
  local pid=$1 command
  command=$(ps -p "$pid" -o command= 2>/dev/null) || return 0
  [[ "$command" == "$bin/consort"* ]] || return 0
  echo "==> stopping $command"
  kill -TERM "$pid" 2>/dev/null || return 0
  for _ in {1..100}; do
    kill -0 "$pid" 2>/dev/null || return 0
    sleep 0.1
  done
  kill -KILL "$pid" 2>/dev/null || true
}
if [[ -f "$pid_file" ]]; then
  while read -r pid; do
    if [[ -n "$pid" ]]; then
      stop_earlier "$pid"
    fi
  done < "$pid_file"
  rm -f "$pid_file"
fi

cleanup() {
  trap - EXIT INT TERM HUP
  if [[ -n "${app_pid:-}" ]]; then
    kill -TERM "$app_pid" 2>/dev/null || true
  fi
  if [[ -n "${hub_pid:-}" ]] && kill -0 "$hub_pid" 2>/dev/null; then
    echo "==> stopping the dev hub"
    # The hub stops its worker and the worker's harness processes. Wait up
    # to 10 seconds for that.
    kill -TERM "$hub_pid" 2>/dev/null || true
    (sleep 10 && kill -KILL "$hub_pid" 2>/dev/null) &
    local killer=$!
    wait "$hub_pid" 2>/dev/null || true
    kill "$killer" 2>/dev/null || true
  fi
  if [[ -n "${tail_pid:-}" ]]; then
    kill "$tail_pid" 2>/dev/null || true
  fi
  rm -f "$pid_file"
}
trap cleanup EXIT
trap 'exit 130' INT TERM HUP

: > "$log"
echo "==> starting the dev hub on $listen (data in $dev_dir/hub)"
"$bin/consort" dev --data "$dev_dir/hub" --listen "$listen" >> "$log" 2>&1 &
hub_pid=$!
echo "$hub_pid" > "$pid_file"

# The hub prints the setup code after its worker connects, or after 10
# seconds when the worker cannot connect.
for _ in {1..300}; do
  grep -q '^Client setup code:' "$log" && break
  if ! kill -0 "$hub_pid" 2>/dev/null; then
    echo "error: the dev hub stopped. The end of its log:" >&2
    tail -n 20 "$log" >&2
    exit 1
  fi
  sleep 0.1
done
if ! grep -q '^Client setup code:' "$log"; then
  echo "error: the dev hub did not start in 30 seconds. Its log is $log." >&2
  exit 1
fi

# Show the log so far, then follow it from that point.
shown=$(wc -c < "$log")
head -c "$shown" "$log"
tail -c +$((shown + 1)) -F "$log" &
tail_pid=$!

if [[ ! -f "$dev_dir/client/client.json" ]]; then
  echo
  echo "    The app is not paired with the dev hub yet. Type these in the app:"
  echo "      Hub URL     $(sed -n 's/^Hub URL: //p' "$log" | tail -n 1)"
  echo "      Setup code  $(sed -n 's/^Client setup code: //p' "$log" | tail -n 1)"
  echo
fi

echo "==> starting the app (log: $log, Ctrl+C stops the app and the hub)"
CONSORT_CONFIG_DIR="$dev_dir/client" "$app_bin" "$@" &
app_pid=$!
echo "$app_pid" >> "$pid_file"
wait "$app_pid" || true
app_pid=
echo "==> the app quit"
