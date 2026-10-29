#!/usr/bin/env bash
# Copies the committed history to the Windows test machine (bigboi) with a git bundle.
# Windows OpenSSH runs commands in cmd.exe, which breaks git push over ssh, so we use a bundle.
set -euo pipefail
HOST="${CONSORT_WINDOWS_HOST:-cawat@bigboi.local}"
REMOTE_DIR='C:/Users/cawat/Projects'
cd "$(git rev-parse --show-toplevel)"
bundle="$(mktemp -t consort).bundle"
git bundle create "$bundle" --all >/dev/null 2>&1
scp -q "$bundle" "$HOST:$REMOTE_DIR/consort.bundle"
rm -f "$bundle"
ssh -o BatchMode=yes "$HOST" "powershell -NoProfile -Command \"Set-Location $REMOTE_DIR; if (Test-Path consort\\.git) { git -C consort fetch -q $REMOTE_DIR/consort.bundle main; git -C consort reset -q --hard FETCH_HEAD } else { git clone -q $REMOTE_DIR/consort.bundle consort }; git -C consort log --oneline -1\""
