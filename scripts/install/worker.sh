#!/usr/bin/env bash
# Installs the Consort worker on macOS or Linux, and pairs it with a hub.
#
#   curl -fsSL https://raw.githubusercontent.com/watzon/consort/main/scripts/install/worker.sh \
#     | bash -s -- --hub https://hub.example.com --code ABCD-EFGH
#
# What it does:
#   1. Finds the release asset for this system on GitHub Releases.
#   2. Downloads it and checks its SHA-256 against the SHA256SUMS file of
#      the same release.
#   3. Puts `consort` in the install folder (default ~/.local/bin) and adds
#      that folder to PATH in your shell profile when it is not there yet.
#   4. Runs `consort worker install`, which pairs the machine with the hub
#      and installs a per-user service (a launchd agent on macOS, a systemd
#      user unit on Linux). It needs no admin rights.
#
# Options:
#   --hub <url>        The hub URL. Required, unless --no-service is set.
#   --code <code>      The pairing code from a client. Without it, the
#                      CONSORT_PAIRING_CODE variable or a prompt gives it.
#   --name <name>      The worker name. The default is the host name.
#   --tag <tag>        A tag for placement. Repeat it for more tags.
#   --version <tag>    A release tag such as v0.2.0. The default is the latest.
#   --no-service       Install the program only. Do not pair or install a service.
#   --dry-run          Print the steps and download nothing.
#   --help             Show this text.
#
# Variables:
#   CONSORT_INSTALL_DIR   Where to put `consort`. Default: ~/.local/bin.
#   CONSORT_DOWNLOAD_BASE The folder URL that holds the release assets. The
#                         tests use it. Default: the GitHub release.
set -euo pipefail

# The one place that names the GitHub repository in this script. The
# PowerShell script, the release workflow and the app have their own copy.
REPO="watzon/consort"

hub=""
code=""
name=""
tags=()
version=""
no_service=0
dry_run=0

say() { printf '%s\n' "$*"; }
fail() {
  printf 'consort install: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'USAGE'
Installs the Consort worker and pairs it with a hub.

Usage: worker.sh --hub <url> --code <code> [options]

  --hub <url>      The hub URL. Required, unless --no-service is set.
  --code <code>    The pairing code from a client.
  --name <name>    The worker name. The default is the host name.
  --tag <tag>      A tag for placement. Repeat it for more tags.
  --version <tag>  A release tag such as v0.2.0. The default is the latest.
  --no-service     Install the program only. Do not pair or install a service.
  --dry-run        Print the steps and download nothing.
  --help           Show this text.
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --hub) [ "$#" -ge 2 ] || fail "--hub needs a URL"; hub="$2"; shift 2 ;;
    --code) [ "$#" -ge 2 ] || fail "--code needs a code"; code="$2"; shift 2 ;;
    --name) [ "$#" -ge 2 ] || fail "--name needs a name"; name="$2"; shift 2 ;;
    --tag) [ "$#" -ge 2 ] || fail "--tag needs a tag"; tags+=("$2"); shift 2 ;;
    --version) [ "$#" -ge 2 ] || fail "--version needs a release tag"; version="$2"; shift 2 ;;
    --no-service) no_service=1; shift ;;
    --dry-run) dry_run=1; shift ;;
    --help | -h) usage; exit 0 ;;
    *) fail "unknown option $1. Run with --help to see the options" ;;
  esac
done

if [ "$no_service" -eq 0 ] && [ -z "$hub" ]; then
  fail "pass --hub with the URL of your hub, or pass --no-service"
fi

# The platform key is the same one that the release workflow puts in the
# asset names: macos-arm64, macos-x86_64, linux-x86_64 or linux-arm64.
case "$(uname -s)" in
  Darwin) os="macos" ;;
  Linux) os="linux" ;;
  *) fail "this script supports macOS and Linux. On Windows, use worker.ps1" ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) arch="arm64" ;;
  x86_64 | amd64) arch="x86_64" ;;
  *) fail "no release for the processor $(uname -m)" ;;
esac
key="${os}-${arch}"
asset="consort-${key}.tar.gz"

if [ -n "${CONSORT_DOWNLOAD_BASE:-}" ]; then
  base="${CONSORT_DOWNLOAD_BASE%/}"
elif [ -n "$version" ]; then
  base="https://github.com/${REPO}/releases/download/${version}"
else
  base="https://github.com/${REPO}/releases/latest/download"
fi

install_dir="${CONSORT_INSTALL_DIR:-$HOME/.local/bin}"

if [ "$dry_run" -eq 1 ]; then
  say "Dry run. Nothing is downloaded or changed."
  say "Download ${base}/${asset}"
  say "Check it against ${base}/SHA256SUMS"
  say "Put consort in ${install_dir}"
  if [ "$no_service" -eq 0 ]; then
    say "Run: consort worker install --hub ${hub} (with the pairing code)"
  fi
  exit 0
fi

command -v tar >/dev/null 2>&1 || fail "tar is not installed"
if command -v curl >/dev/null 2>&1; then
  download() { curl -fsSL --retry 3 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  download() { wget -q -O "$2" "$1"; }
else
  fail "curl or wget must be installed"
fi
if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
  fail "sha256sum or shasum must be installed"
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

say "Downloading ${asset}"
download "${base}/${asset}" "${work}/${asset}" || fail "could not download ${base}/${asset}. Check the version and your network"
download "${base}/SHA256SUMS" "${work}/SHA256SUMS" || fail "could not download ${base}/SHA256SUMS"

# A line of SHA256SUMS is "<hash>  <file name>".
expected="$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1; exit }' "${work}/SHA256SUMS")"
[ -n "$expected" ] || fail "SHA256SUMS has no entry for ${asset}"
actual="$(sha256 "${work}/${asset}")"
if [ "$expected" != "$actual" ]; then
  fail "the checksum of ${asset} does not match. Expected ${expected}, got ${actual}. Nothing was installed"
fi
say "Checksum is correct."

mkdir -p "${work}/unpacked"
tar -xzf "${work}/${asset}" -C "${work}/unpacked"
[ -f "${work}/unpacked/consort" ] || fail "${asset} has no consort program"
mkdir -p "$install_dir"
# Write to a new name and rename it, so a running worker keeps its old file
# until the rename, and a failed copy leaves the old file in place.
cp "${work}/unpacked/consort" "${install_dir}/consort.new"
chmod 755 "${install_dir}/consort.new"
mv -f "${install_dir}/consort.new" "${install_dir}/consort"
say "Installed ${install_dir}/consort"

on_path=0
case ":${PATH}:" in
  *":${install_dir}:"*) on_path=1 ;;
esac
if [ "$on_path" -eq 0 ]; then
  case "$(basename "${SHELL:-sh}")" in
    zsh) profile="$HOME/.zshrc" ;;
    bash)
      if [ -f "$HOME/.bashrc" ]; then profile="$HOME/.bashrc"; else profile="$HOME/.bash_profile"; fi
      ;;
    *) profile="$HOME/.profile" ;;
  esac
  line="export PATH=\"${install_dir}:\$PATH\""
  if ! grep -qsF "$line" "$profile"; then
    printf '\n# Added by the Consort installer\n%s\n' "$line" >>"$profile"
    say "Added ${install_dir} to PATH in ${profile}. Open a new terminal to use it."
  fi
fi

if [ "$no_service" -eq 1 ]; then
  say "Done. Pair this machine with: consort worker install --hub <url> --code <code>"
  exit 0
fi

args=(worker install --hub "$hub")
[ -n "$name" ] && args+=(--name "$name")
for tag in "${tags[@]+"${tags[@]}"}"; do
  args+=(--tag "$tag")
done
# The code goes in the environment, so other users of this machine do not
# see it in the process list.
if [ -n "$code" ]; then
  CONSORT_PAIRING_CODE="$code" "${install_dir}/consort" "${args[@]}"
else
  # With no code the machine must be paired already. The script may come
  # from a pipe, so the program must not read its input as a prompt answer.
  "${install_dir}/consort" "${args[@]}" </dev/null
fi
