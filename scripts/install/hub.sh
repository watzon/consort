#!/usr/bin/env bash
# Installs a Consort hub and its session host on a Linux server, as two
# systemd services, and prints the address and a setup code for the app.
#
#   curl -fsSL https://raw.githubusercontent.com/watzon/consort/main/scripts/install/hub.sh | sudo bash
#   curl -fsSL https://raw.githubusercontent.com/watzon/consort/main/scripts/install/hub.sh | sudo bash -s -- --domain hub.example.com
#
# This script has never run on a real server in the build that wrote it. It is
# tested with --dry-run, with shellcheck, and by sourcing its functions. Read
# docs/deploy.md for the same steps by hand.
#
# What it does:
#   1. Needs Linux (x86_64 or arm64), root and systemd. Run it as root. As
#      another user, it runs itself again through sudo (it reads the script
#      once more from GitHub when it comes from a pipe).
#   2. Installs bubblewrap and git with apt or dnf when they are missing. It
#      also installs the libraries and fonts of the preview browser, with the
#      names of Debian and Ubuntu (or of Fedora). The session host downloads the
#      browser itself on first use. A failure here is a warning: a visual has no
#      image, and the rest works.
#   3. Downloads the release for this system and checks its SHA-256 against the
#      SHA256SUMS file of the same release. A wrong checksum stops the script
#      before it changes anything.
#   4. Creates the users `consort` (the hub) and `consort-session` (the session
#      host), puts `consort` in /usr/local/bin, and installs and starts the units
#      consort-hub and consort-session-host. The data stays in /var/lib/consort
#      and /var/lib/consort-session-host.
#   5. Gives the hub an address. With Tailscale it runs `consort hub reach
#      tailscale`. With --domain it installs Caddy, which gets the certificate.
#   6. Pairs the session host with the hub, checks that bubblewrap can make a
#      user namespace, adds an AppArmor profile for the preview browser when
#      AppArmor limits user namespaces, and prints the address and a new setup
#      code.
#
# It is safe to run again. A second run keeps the data, replaces the program
# and the units when they differ, and restarts the services when it changed
# something. It never turns on Tailscale Funnel and never runs `tailscale serve
# reset`.
#
# Options:
#   --domain <name>    Use a domain name and Caddy, not Tailscale. The name must
#                      point at this server, and ports 80 and 443 must be open.
#   --version <tag>    A release tag such as v0.2.0. The default is the latest.
#   --dry-run          Print the steps and change nothing. Needs no root.
#   --help             Show this text.
#
# Variables:
#   CONSORT_DOWNLOAD_BASE  The folder URL that holds the release assets. The
#                          tests use it. Default: the GitHub release.
set -euo pipefail

# The one place that names the GitHub repository in this script. The other
# install scripts, the release workflow and the app have their own copy.
REPO="watzon/consort"

SCRIPT_URL="https://raw.githubusercontent.com/${REPO}/main/scripts/install/hub.sh"
HUB_URL_LOCAL="http://127.0.0.1:7600"
INSTALL_BIN="/usr/local/bin/consort"
HUB_DATA="/var/lib/consort"
HOST_DATA="/var/lib/consort-session-host"
ENV_DIR="/etc/consort"
HUB_ENV="${ENV_DIR}/hub.env"
UNIT_DIR="/etc/systemd/system"
CADDY_DIR="/etc/caddy"
# A line in hub.env that tells a second run which address this script chose.
MARK="# consort-install address="
APPARMOR_DIR="/etc/apparmor.d"
USERNS_RESTRICT_FILE="/proc/sys/kernel/apparmor_restrict_unprivileged_userns"

# The libraries and fonts of the preview browser (chrome-headless-shell). The
# session host downloads the browser, and draws the image of each visual with
# it. The same list is in docs/deploy.md and in deploy/session-host/Dockerfile.
# Debian 13 and Ubuntu 24.04 renamed some libraries with a "t64" ending, and
# `deb_pick` finds the name that a system has. The names for dnf are checked on
# Fedora only.
BROWSER_DEB_PACKAGES=(
  libasound2 libatk-bridge2.0-0 libatk1.0-0 libatspi2.0-0 libdbus-1-3 libexpat1
  libgbm1 libglib2.0-0 libnspr4 libnss3 libx11-6 libxcb1 libxcomposite1
  libxdamage1 libxext6 libxfixes3 libxkbcommon0 libxrandr2 fontconfig fonts-liberation
)
BROWSER_RPM_PACKAGES=(
  alsa-lib atk at-spi2-atk at-spi2-core dbus-libs expat mesa-libgbm glib2 nspr nss
  libX11 libxcb libXcomposite libXdamage libXext libXfixes libxkbcommon libXrandr
  fontconfig liberation-fonts
)

domain=""
version=""
dry_run=0
orig_args=()
work=""
hub_address=""
address_failed=0

say() { printf '%s\n' "$*"; }
warn() { printf 'consort install: %s\n' "$*" >&2; }
fail() {
  warn "$*"
  exit 1
}

usage() {
  cat <<'USAGE'
Installs a Consort hub and its session host on a Linux server.

Usage: sudo bash hub.sh [options]

  --domain <name>  Use a domain name and Caddy, not Tailscale.
  --version <tag>  A release tag such as v0.2.0. The default is the latest.
  --dry-run        Print the steps and change nothing. Needs no root.
  --help           Show this text.

With no option, the script uses Tailscale. Tailscale must run on this server,
with HTTPS certificates turned on for your tailnet.
USAGE
}

parse_args() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --domain) [ "$#" -ge 2 ] || fail "--domain needs a name"; domain="$2"; shift 2 ;;
      --version) [ "$#" -ge 2 ] || fail "--version needs a release tag"; version="$2"; shift 2 ;;
      --dry-run) dry_run=1; shift ;;
      --help | -h) usage; exit 0 ;;
      *) fail "unknown option $1. Run with --help to see the options" ;;
    esac
  done
  if [ -n "$domain" ]; then
    case "$domain" in
      *://* | */* | *:* | *" "*) fail "pass the name only, such as --domain hub.example.com" ;;
      *.*) ;;
      *) fail "--domain needs a full name such as hub.example.com" ;;
    esac
    case "$domain" in
      *[!A-Za-z0-9.-]* | .* | *. | -*) fail "the name $domain has a character that a domain name cannot have" ;;
    esac
    domain="$(printf '%s' "$domain" | tr '[:upper:]' '[:lower:]')"
  fi
}

# ---- Embedded files. A test checks that they equal the files in deploy/. ----

unit_hub() {
  cat <<'CONSORT_UNIT_EOF'
# The Consort hub as a hardened systemd service.
#
# Install with scripts/install/hub.sh, or by hand (see docs/deploy.md):
#   sudo useradd --system --home-dir /var/lib/consort --shell /usr/sbin/nologin consort
#   sudo install -m 0755 consort /usr/local/bin/consort
#   sudo install -m 0644 deploy/hub/consort-hub.service /etc/systemd/system/
#   sudo systemctl daemon-reload
#   sudo systemctl enable --now consort-hub
#
# The hub listens on 127.0.0.1 only. A reverse proxy (Caddy, see
# deploy/hub/Caddyfile) serves HTTPS and passes WebSocket traffic to it, or
# Tailscale Serve does (`consort hub reach tailscale`).
#
# The public URL below is a placeholder. Set the real one in
# /etc/consort/hub.env, which overrides it:
#   CONSORT_PUBLIC_URL=https://hub.example.com
# With Tailscale, use CONSORT_PUBLIC_URL=http://127.0.0.1:7600. The address on
# the tailnet is a reach address, and the hub accepts it without a restart.
[Unit]
Description=Consort hub
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=consort
Group=consort
# systemd makes /var/lib/consort, owned by the user above, and keeps it
# writable under ProtectSystem=strict. The hub keeps hub.db and blobs/ there.
StateDirectory=consort
StateDirectoryMode=0700
WorkingDirectory=/var/lib/consort
Environment=CONSORT_HUB_DATA=/var/lib/consort
Environment=CONSORT_HUB_LISTEN=127.0.0.1:7600
Environment=CONSORT_PUBLIC_URL=https://hub.example.com
# The hub reports this in hub.status. Settings > Hub uses it to show the update
# command.
Environment=CONSORT_INSTALL_KIND=service
# Extra settings, such as RUST_LOG=debug. The file is optional.
EnvironmentFile=-/etc/consort/hub.env
ExecStart=/usr/local/bin/consort hub
Restart=on-failure
RestartSec=5
TimeoutStopSec=30

# Hardening. The hub needs the network, its data folder and `git` (for skill
# sources). It needs nothing else.
NoNewPrivileges=true
CapabilityBoundingSet=
AmbientCapabilities=
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
PrivateDevices=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectKernelLogs=true
ProtectControlGroups=true
ProtectClock=true
ProtectHostname=true
ProtectProc=invisible
ProcSubset=pid
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictNamespaces=true
RestrictRealtime=true
RestrictSUIDSGID=true
LockPersonality=true
MemoryDenyWriteExecute=true
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallFilter=~@privileged @resources
UMask=0077

[Install]
WantedBy=multi-user.target
CONSORT_UNIT_EOF
}

unit_session_host() {
  cat <<'CONSORT_UNIT_EOF'
# The Consort session host as a systemd service, on the hub machine.
#
# It holds the accounts and runs the harness CLIs. It is a separate service
# because consort-hub.service sets ProtectHome, RestrictNamespaces and
# MemoryDenyWriteExecute, and a CLI runtime and bubblewrap cannot run under
# those settings. It has its own user, so it cannot read hub.db, and the hub
# cannot read the profiles of the accounts.
#
# Install with scripts/install/hub.sh, or by hand (see docs/deploy.md):
#   sudo apt install bubblewrap git
#   sudo apt install <the libraries of the preview browser, in docs/deploy.md>
#   sudo useradd --system --home-dir /var/lib/consort-session-host --shell /usr/sbin/nologin consort-session
#   sudo install -m 0644 deploy/session-host/consort-session-host.service /etc/systemd/system/
#   sudo -u consort consort hub session-host-code --data /var/lib/consort
#   sudo -u consort-session consort session-host pair <public URL of the hub> --code <code> \
#     --config-dir /var/lib/consort-session-host/config
#   sudo systemctl daemon-reload
#   sudo systemctl enable --now consort-session-host
#
# Pair with the public URL of the hub (CONSORT_PUBLIC_URL), exactly. The session
# host signs that address, and the hub accepts only its public URL and the
# addresses of its reach.
#
# A session does not start when bubblewrap cannot run. On Ubuntu 24.04 and later,
# AppArmor can stop bubblewrap from making a user namespace. Add a profile for
# /usr/bin/bwrap with the `userns` rule, or set
# kernel.apparmor_restrict_unprivileged_userns=0. The preview browser that draws
# the images of visuals needs the same: a profile with the `userns` rule for
# /var/lib/consort-session-host/home/tools/chrome-headless-shell/*/chrome-headless-shell.
[Unit]
Description=Consort session host
After=network-online.target consort-hub.service
Wants=network-online.target

[Service]
Type=simple
User=consort-session
Group=consort-session
StateDirectory=consort-session-host
StateDirectoryMode=0700
WorkingDirectory=/var/lib/consort-session-host
Environment=HOME=/var/lib/consort-session-host
EnvironmentFile=-/etc/consort/session-host.env
ExecStart=/usr/local/bin/consort session-host run \
  --config-dir /var/lib/consort-session-host/config \
  --consort-home /var/lib/consort-session-host/home
Restart=on-failure
RestartSec=5
# A restart interrupts the sessions. The CLIs stop with the process group, and
# the session host resumes them after it checks the open operations.
KillMode=control-group
TimeoutStopSec=30

# The hardening that a CLI runtime and bubblewrap allow. There is no
# ProtectHome, RestrictNamespaces or MemoryDenyWriteExecute here: Node needs
# writable and executable memory, and bubblewrap needs namespaces.
ProtectSystem=strict
ProtectHome=read-only
PrivateTmp=true
PrivateDevices=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectKernelLogs=true
ProtectControlGroups=true
ProtectClock=true
RestrictRealtime=true
RestrictSUIDSGID=true
LockPersonality=true
SystemCallArchitectures=native

[Install]
WantedBy=multi-user.target
CONSORT_UNIT_EOF
}

# The Caddyfile of deploy/hub/Caddyfile. `caddy_site` puts the domain in it.
caddy_template() {
  cat <<'CONSORT_UNIT_EOF'
# Caddy in front of the Consort hub. Caddy gets and renews the certificate
# for the name below by itself. Point the DNS record of the name at this
# machine, and open ports 80 and 443.
#
# Caddy passes WebSocket traffic with no extra setting. The hub also needs
# no special header from the proxy.
hub.example.com {
	encode zstd gzip

	# The hub serves the client link (/v1/client), the worker link
	# (/v1/worker), pairing, blobs and /healthz.
	reverse_proxy 127.0.0.1:7600 {
		# Uploads and downloads of attachments can be 256 MiB. A WebSocket
		# stays open for as long as the app runs.
		flush_interval -1
		transport http {
			read_timeout 0
			write_timeout 0
		}
	}
}
CONSORT_UNIT_EOF
}

caddy_site() {
  caddy_template | sed "s/hub\\.example\\.com/${1}/g"
}

# ---- Small helpers. ----

# Runs a command as another user.
as_user() {
  local user="$1"
  shift
  if command -v runuser >/dev/null 2>&1; then
    runuser -u "$user" -- "$@"
  else
    sudo -u "$user" -- "$@"
  fi
}

http_get() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsS --max-time 5 "$1"
  else
    wget -q -T 5 -O - "$1"
  fi
}

# Waits until an address answers. Usage: wait_for <url> <seconds>
wait_for() {
  local url="$1" left="$2"
  while [ "$left" -gt 0 ]; do
    if http_get "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
    left=$((left - 2))
  done
  return 1
}

# The setup code in the output of `consort hub setup-code`.
parse_setup_code() {
  awk '/code:/ { print $NF; exit }'
}

# The address in the output of `consort hub reach tailscale`.
parse_reach_address() {
  sed -n 's/^The hub answers at \(.*\)\.$/\1/p' | head -n 1
}

# The release asset for this system. Prints "linux-x86_64" or "linux-arm64".
system_key() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  # Only a dry run takes the system from the variables. The tests use them.
  if [ "$dry_run" -eq 1 ]; then
    os="${CONSORT_DRY_RUN_OS:-$os}"
    arch="${CONSORT_DRY_RUN_ARCH:-$arch}"
  fi
  case "$os" in
    Linux) ;;
    Darwin) fail "this script installs a hub on a Linux server. On a Mac, open the Consort app and choose On this computer" ;;
    *) fail "this script installs a hub on a Linux server. It does not support $os" ;;
  esac
  case "$arch" in
    x86_64 | amd64) printf 'linux-x86_64\n' ;;
    aarch64 | arm64) printf 'linux-arm64\n' ;;
    *) fail "no release has a program for the processor $arch. Build Consort from source" ;;
  esac
}

release_base() {
  if [ -n "${CONSORT_DOWNLOAD_BASE:-}" ]; then
    printf '%s\n' "${CONSORT_DOWNLOAD_BASE%/}"
  elif [ -n "$version" ]; then
    printf 'https://github.com/%s/releases/download/%s\n' "$REPO" "$version"
  else
    printf 'https://github.com/%s/releases/latest/download\n' "$REPO"
  fi
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  else
    fail "sha256sum must be installed"
  fi
}

download() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 3 -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$2" "$1"
  else
    fail "curl or wget must be installed"
  fi
}

# Downloads the release for a system key, checks its SHA-256, and leaves the
# program in <work>/consort. On any failure it stops and leaves the system
# untouched: it writes only inside <work>. Usage: fetch_program <work> <key> <base>
fetch_program() {
  local work="$1" key="$2" base="$3" asset expected actual
  asset="consort-${key}.tar.gz"
  command -v tar >/dev/null 2>&1 || fail "tar is not installed. Install it, then run this script again"
  say "Downloading ${asset}"
  download "${base}/${asset}" "${work}/${asset}" || fail "could not download ${base}/${asset}. Check the version and the network of this server"
  download "${base}/SHA256SUMS" "${work}/SHA256SUMS" || fail "could not download ${base}/SHA256SUMS"
  # A line of SHA256SUMS is "<hash>  <file name>".
  expected="$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1; exit }' "${work}/SHA256SUMS")"
  [ -n "$expected" ] || fail "SHA256SUMS has no entry for ${asset}. Nothing was installed"
  actual="$(sha256_of "${work}/${asset}")"
  if [ "$expected" != "$actual" ]; then
    fail "the checksum of ${asset} does not match. Expected ${expected}, got ${actual}. Nothing was installed"
  fi
  say "Checksum is correct."
  mkdir -p "${work}/unpacked"
  tar -xzf "${work}/${asset}" -C "${work}/unpacked"
  [ -f "${work}/unpacked/consort" ] || fail "${asset} has no consort program. Nothing was installed"
  cp "${work}/unpacked/consort" "${work}/consort"
  chmod 755 "${work}/consort"
}

# ---- Checks before any change. ----

require_root() {
  if [ "$(id -u)" -eq 0 ]; then
    return 0
  fi
  local self="${BASH_SOURCE[0]:-}" text tmp
  if ! command -v sudo >/dev/null 2>&1; then
    fail "this script must run as root, and sudo is not installed. Sign in as root, then run it again"
  fi
  say "This script needs root. Running it again with sudo."
  if [ -f "$self" ]; then
    exec sudo bash "$self" "${orig_args[@]+"${orig_args[@]}"}"
  fi
  # The script came from a pipe, so it cannot start itself again. Read the same
  # script once more, and give it to sudo.
  tmp="$(mktemp)"
  if ! download "$SCRIPT_URL" "$tmp"; then
    rm -f "$tmp"
    fail "could not download ${SCRIPT_URL}. Run it again with sudo: curl -fsSL ${SCRIPT_URL} | sudo bash"
  fi
  text="$(cat "$tmp")"
  rm -f "$tmp"
  exec sudo bash -c "$text" hub.sh "${orig_args[@]+"${orig_args[@]}"}"
}

require_systemd() {
  if ! command -v systemctl >/dev/null 2>&1 || [ ! -d /run/systemd/system ]; then
    fail "this server does not run systemd, so the services cannot start. Use Docker, or run the programs by hand. See docs/deploy.md"
  fi
}

tailscale_ready() {
  command -v tailscale >/dev/null 2>&1 || return 1
  tailscale status --json 2>/dev/null | grep -Eq '"BackendState"[[:space:]]*:[[:space:]]*"Running"'
}

# The address choice that a first run records in hub.env. Sets address_mode to
# "domain", "tailscale" or "kept", and domain to the name.
address_mode=""
fresh=1
choose_address() {
  if [ -e "${UNIT_DIR}/consort-hub.service" ] && [ -e "${HUB_DATA}/hub.db" ]; then
    fresh=0
  fi
  if [ -n "$domain" ]; then
    address_mode="domain"
    return 0
  fi
  local mark=""
  if [ -f "$HUB_ENV" ]; then
    mark="$(sed -n "s/^${MARK}//p" "$HUB_ENV" | tail -n 1)"
  fi
  case "$mark" in
    domain\ *)
      address_mode="domain"
      domain="${mark#domain }"
      return 0
      ;;
    tailscale)
      address_mode="tailscale"
      return 0
      ;;
  esac
  if [ "$fresh" -eq 0 ]; then
    # An earlier install by hand set the address. Keep it.
    address_mode="kept"
    return 0
  fi
  if tailscale_ready; then
    address_mode="tailscale"
    return 0
  fi
  explain_choices
  exit 1
}

explain_choices() {
  local why="Tailscale is not on this server."
  if command -v tailscale >/dev/null 2>&1; then
    why="Tailscale is on this server, but it is not connected."
  fi
  cat >&2 <<EOF
consort install: the hub needs an address, and none is ready. ${why}

Choose one of two ways, then run this script again:

  1. Tailscale (recommended). It needs no domain name and no open port.
     Install Tailscale on this server and sign in: tailscale up
     Then turn on HTTPS certificates for your tailnet, one time, in the Tailscale
     admin console (the DNS page). Then run this script with no option.

  2. A domain name. Point an A or AAAA record of the name at this server, and
     open ports 80 and 443. Then run:
     curl -fsSL https://raw.githubusercontent.com/${REPO}/main/scripts/install/hub.sh | sudo bash -s -- --domain hub.example.com
EOF
}

# ---- Steps. ----

apt_updated=0
apt_refresh() {
  if [ "$apt_updated" -eq 0 ]; then
    DEBIAN_FRONTEND=noninteractive apt-get update -qq >/dev/null || return 1
    apt_updated=1
  fi
}

install_packages() {
  local need=()
  command -v bwrap >/dev/null 2>&1 || need+=(bubblewrap)
  command -v git >/dev/null 2>&1 || need+=(git)
  if [ "${#need[@]}" -gt 0 ]; then
    say "Installing ${need[*]}"
    if command -v apt-get >/dev/null 2>&1; then
      apt_refresh
      DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "${need[@]}" >/dev/null
    elif command -v dnf >/dev/null 2>&1; then
      dnf install -y -q "${need[@]}" >/dev/null
    else
      fail "${need[*]} must be installed, and this script knows only apt and dnf. Install ${need[*]} with the package manager of this server, then run this script again"
    fi
    command -v bwrap >/dev/null 2>&1 || fail "bubblewrap did not install. Install it by hand, then run this script again"
    command -v git >/dev/null 2>&1 || fail "git did not install. Install it by hand, then run this script again"
  fi
  install_browser_packages
}

# True when a Debian package is installed. Usage: deb_installed <name>
deb_installed() {
  # shellcheck disable=SC2016
  [ "$(dpkg-query -W -f='${db:Status-Status}' "$1" 2>/dev/null)" = "installed" ]
}

# The candidate version of a package that apt knows, or nothing. A virtual
# package, such as libasound2 on Ubuntu 24.04, has no candidate.
deb_candidate() {
  apt-cache policy "$1" 2>/dev/null | awk '$1 == "Candidate:" && $2 != "(none)" { print $2; exit }'
}

# The name that this system uses for a library: the name itself, or the name
# with the "t64" ending of Debian 13 and Ubuntu 24.04. Run `apt-get update`
# first. Usage: deb_pick libasound2
deb_pick() {
  if [ -n "$(deb_candidate "$1")" ]; then
    printf '%s\n' "$1"
  elif [ -n "$(deb_candidate "${1}t64")" ]; then
    printf '%s\n' "${1}t64"
  else
    printf '%s\n' "$1"
  fi
}

# Puts the packages of the preview browser that are not installed in
# browser_missing. A Debian name counts as installed when its t64 name is.
browser_missing=()
find_missing_browser_packages() {
  local name
  browser_missing=()
  if command -v apt-get >/dev/null 2>&1 && command -v dpkg-query >/dev/null 2>&1; then
    for name in "${BROWSER_DEB_PACKAGES[@]}"; do
      if ! deb_installed "$name" && ! deb_installed "${name}t64"; then
        browser_missing+=("$name")
      fi
    done
  elif command -v dnf >/dev/null 2>&1 && command -v rpm >/dev/null 2>&1; then
    for name in "${BROWSER_RPM_PACKAGES[@]}"; do
      # `--whatprovides`, because liberation-fonts is a name that a package provides.
      rpm -q --quiet --whatprovides "$name" 2>/dev/null || browser_missing+=("$name")
    done
  fi
}

# Installs the libraries and fonts of the preview browser. A failure is a
# warning and not an error: the hub works, but a visual has no image.
install_browser_packages() {
  local want=() name
  find_missing_browser_packages
  if [ "${#browser_missing[@]}" -eq 0 ]; then
    return 0
  fi
  say "Installing the libraries and fonts of the preview browser"
  if command -v apt-get >/dev/null 2>&1; then
    if ! apt_refresh; then
      warn "apt could not update its package lists, so the libraries of the preview browser are not installed. A visual has no image until you install them. See docs/deploy.md"
      return 0
    fi
    for name in "${browser_missing[@]}"; do
      want+=("$(deb_pick "$name")")
    done
    if DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "${want[@]}" >/dev/null; then
      return 0
    fi
  else
    want=("${browser_missing[@]}")
    if dnf install -y -q "${want[@]}" >/dev/null; then
      return 0
    fi
  fi
  warn "could not install all the libraries of the preview browser, so a visual has no image. Install these by hand: ${want[*]}. See docs/deploy.md"
  return 0
}

nologin_shell() {
  local shell
  for shell in /usr/sbin/nologin /sbin/nologin /bin/false; do
    if [ -x "$shell" ]; then
      printf '%s\n' "$shell"
      return 0
    fi
  done
  printf '/bin/false\n'
}

ensure_user() {
  local name="$1" home="$2"
  if id "$name" >/dev/null 2>&1; then
    return 0
  fi
  say "Creating the user ${name}"
  useradd --system --no-create-home --home-dir "$home" --shell "$(nologin_shell)" "$name"
}

# Counts as a change when the file differs. Usage: install_unit <name> <function>
changed=0
install_unit() {
  local name="$1" make="$2" path tmp
  path="${UNIT_DIR}/${name}"
  tmp="$(mktemp)"
  "$make" >"$tmp"
  if [ -f "$path" ] && cmp -s "$tmp" "$path"; then
    rm -f "$tmp"
    return 0
  fi
  if [ -f "$path" ]; then
    cp -p "$path" "${path}.old"
    say "Replaced ${path}. The old file is in ${path}.old"
  fi
  install -m 0644 "$tmp" "$path"
  rm -f "$tmp"
  changed=1
}

# Sets KEY=VALUE in hub.env. The line stays where it was, and every other line
# stays as it is, so a second run with the same value changes nothing.
# Usage: set_env_line <key> <value>
set_env_line() {
  local key="$1" value="$2" tmp
  tmp="$(mktemp)"
  if [ -f "$HUB_ENV" ]; then
    awk -v key="$key" -v value="$value" '
      index($0, key "=") == 1 { if (!done) { print key "=" value; done = 1 } next }
      { print }
      END { if (!done) print key "=" value }
    ' "$HUB_ENV" >"$tmp"
  else
    printf '# Settings for consort-hub. The install script keeps the other lines of this file.\n%s=%s\n' "$key" "$value" >"$tmp"
  fi
  install -d -m 0755 "$ENV_DIR"
  if [ -f "$HUB_ENV" ] && cmp -s "$tmp" "$HUB_ENV"; then
    rm -f "$tmp"
    return 0
  fi
  install -m 0644 "$tmp" "$HUB_ENV"
  rm -f "$tmp"
  changed=1
}

write_address_settings() {
  case "$address_mode" in
    domain)
      set_env_line CONSORT_PUBLIC_URL "https://${domain}"
      set_env_line "${MARK%=}" "domain ${domain}"
      ;;
    tailscale)
      # The hub stays on loopback. The address on the tailnet is a reach
      # address, and a device signs it. The public URL is the address that the
      # session host on this machine uses.
      set_env_line CONSORT_PUBLIC_URL "$HUB_URL_LOCAL"
      set_env_line "${MARK%=}" "tailscale"
      ;;
    *) ;;
  esac
}

# The public URL that the hub uses now.
current_public_url() {
  local url=""
  if [ -f "$HUB_ENV" ]; then
    url="$(sed -n 's/^CONSORT_PUBLIC_URL=//p' "$HUB_ENV" | tail -n 1)"
  fi
  if [ -z "$url" ]; then
    url="$(systemctl show consort-hub --property=Environment --value 2>/dev/null | tr ' ' '\n' | sed -n 's/^CONSORT_PUBLIC_URL=//p' | tail -n 1)"
  fi
  printf '%s\n' "${url:-$HUB_URL_LOCAL}"
}

own_data() {
  chown -R consort:consort "$HUB_DATA" 2>/dev/null || true
}

start_service() {
  local name="$1" restart="$2"
  systemctl enable "$name" >/dev/null 2>&1 || true
  if [ "$restart" -eq 1 ] && systemctl is-active --quiet "$name"; then
    say "Restarting ${name}"
    systemctl restart "$name"
  else
    systemctl start "$name"
  fi
}

setup_tailscale() {
  if ! tailscale_ready; then
    if [ "$fresh" -eq 1 ]; then
      explain_choices
      exit 1
    fi
    warn "Tailscale is not connected, so this run leaves the address as it was. Run tailscale up, then run this script again."
    return 0
  fi
  local out address
  if ! out="$("$INSTALL_BIN" hub reach tailscale --data "$HUB_DATA" 2>&1)"; then
    own_data
    printf '%s\n' "$out" >&2
    fail "Tailscale did not give the hub an address. The hub runs on this server, with no address for other devices. Fix the problem in the message above, then run this script again"
  fi
  own_data
  address="$(printf '%s\n' "$out" | parse_reach_address)"
  if [ -z "$address" ]; then
    printf '%s\n' "$out"
    address="(run: consort hub reach status --data ${HUB_DATA})"
  fi
  hub_address="$address"
}

install_caddy_package() {
  if command -v apt-get >/dev/null 2>&1; then
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq caddy >/dev/null
  elif command -v dnf >/dev/null 2>&1; then
    dnf install -y -q caddy
  else
    return 1
  fi
}

caddy_help() {
  cat >&2 <<EOF
consort install: Caddy is not ready on this server. Install it by hand:
  1. Install Caddy: https://caddyserver.com/docs/install
  2. Add this line to ${CADDY_DIR}/Caddyfile:  import ${CADDY_DIR}/consort.caddy
  3. Run: systemctl enable --now caddy && systemctl reload caddy
Then run this script again.
EOF
}

setup_caddy() {
  if ! getent hosts "$domain" >/dev/null 2>&1; then
    warn "the name ${domain} has no DNS record yet. Caddy cannot get a certificate until it has one."
  fi
  if ! command -v caddy >/dev/null 2>&1; then
    say "Installing Caddy"
    install_caddy_package || true
  fi
  install -d -m 0755 "$CADDY_DIR"
  caddy_site "$domain" >"${CADDY_DIR}/consort.caddy"
  if ! command -v caddy >/dev/null 2>&1; then
    caddy_help
    return 1
  fi
  local line="import ${CADDY_DIR}/consort.caddy" file="${CADDY_DIR}/Caddyfile"
  if ! grep -qsxF "$line" "$file"; then
    # Add one line. Another site in the file stays as it is.
    [ -f "$file" ] && cp -p "$file" "${file}.before-consort"
    printf '\n%s\n' "$line" >>"$file"
  fi
  if ! caddy validate --config "$file" --adapter caddyfile >/dev/null 2>&1; then
    [ -f "${file}.before-consort" ] && cp -p "${file}.before-consort" "$file"
    warn "Caddy does not accept its configuration. The old Caddyfile is back in place. Run: caddy validate --config ${file}"
    return 1
  fi
  systemctl enable caddy >/dev/null 2>&1 || true
  systemctl start caddy
  systemctl reload caddy 2>/dev/null || systemctl restart caddy
  return 0
}

# True when AppArmor stops a program from making a user namespace, unless a
# profile with the userns rule allows it (Ubuntu 24.04 and later).
apparmor_restricts_userns() {
  [ "$(cat "$USERNS_RESTRICT_FILE" 2>/dev/null || printf 0)" = "1" ]
}

# The text of an AppArmor profile that lets one program make a user namespace.
# Usage: userns_profile_text <profile name> <path of the program>
userns_profile_text() {
  if [ -e "${APPARMOR_DIR}/abi/4.0" ]; then
    printf 'abi <abi/4.0>,\n'
  fi
  printf 'include <tunables/global>\n\n'
  printf 'profile %s %s flags=(unconfined) {\n  userns,\n\n' "$1" "$2"
  printf '  include if exists <local/%s>\n}\n' "$1"
}

# bubblewrap must make a user namespace for the user of the session host. On
# Ubuntu 24.04 and later, AppArmor can stop it. It is a warning and not an
# error: the hub works, but no session starts.
check_boundary() {
  local out bwrap_path
  if out="$(as_user consort-session bwrap --unshare-user --ro-bind / / true 2>&1)"; then
    say "bubblewrap can make a user namespace."
    return 0
  fi
  bwrap_path="$(readlink -f "$(command -v bwrap)")"
  if apparmor_restricts_userns && command -v apparmor_parser >/dev/null 2>&1 && [ -d "$APPARMOR_DIR" ]; then
    say "AppArmor stops bubblewrap from making a user namespace. Adding a profile for ${bwrap_path}."
    userns_profile_text consort-bwrap "$bwrap_path" >"${APPARMOR_DIR}/consort-bwrap"
    if apparmor_parser -r "${APPARMOR_DIR}/consort-bwrap" 2>/dev/null \
      && as_user consort-session bwrap --unshare-user --ro-bind / / true >/dev/null 2>&1; then
      say "bubblewrap works now."
      return 0
    fi
    rm -f "${APPARMOR_DIR}/consort-bwrap"
  fi
  warn "bubblewrap cannot make a user namespace for the session host, so no session will start. The error was: ${out}"
  warn "On Ubuntu 24.04 and later, add an AppArmor profile for ${bwrap_path} with the userns rule, or run: sysctl -w kernel.apparmor_restrict_unprivileged_userns=0. See docs/deploy.md. Then run: systemctl restart consort-session-host"
  return 0
}

# The path of the preview browser in the tools folder of the session host. The
# `*` is the version. The session host puts it there on first use.
browser_path_glob() {
  printf '%s/home/tools/chrome-headless-shell/*/chrome-headless-shell\n' "$HOST_DATA"
}

# Writes the AppArmor profile of the preview browser when it is not there yet,
# and loads it. Returns 1 when it cannot.
ensure_browser_profile() {
  local file="${APPARMOR_DIR}/consort-chrome-headless-shell" tmp
  tmp="$(mktemp)"
  userns_profile_text consort-chrome-headless-shell "$(browser_path_glob)" >"$tmp"
  if [ -f "$file" ] && cmp -s "$tmp" "$file"; then
    rm -f "$tmp"
    return 0
  fi
  install -m 0644 "$tmp" "$file"
  rm -f "$tmp"
  if ! apparmor_parser -r "$file" 2>/dev/null; then
    rm -f "$file"
    return 1
  fi
}

# The browser draws the image of a visual in a sandbox that needs a user
# namespace, so AppArmor can stop it in the same way as bubblewrap. The browser
# downloads on first use, so it is often not there yet. This step writes the
# profile before that, and runs the browser when it is there. It is a warning
# and not an error: the hub works, but a visual has no image.
check_browser() {
  local exe="" candidate out tmp limit=()
  if apparmor_restricts_userns; then
    if command -v apparmor_parser >/dev/null 2>&1 && [ -d "$APPARMOR_DIR" ]; then
      if ensure_browser_profile; then
        say "AppArmor limits user namespaces. A profile lets the preview browser make its sandbox."
      else
        warn "could not load an AppArmor profile for the preview browser, so a visual may have no image. See docs/deploy.md"
      fi
    else
      warn "AppArmor limits user namespaces, and apparmor_parser is not installed, so the preview browser may not start. Install the apparmor package, then run this script again. See docs/deploy.md"
    fi
  fi
  # One folder for each version. The last one in the list is the one to test.
  for candidate in "${HOST_DATA}"/home/tools/chrome-headless-shell/*/chrome-headless-shell; do
    if [ -x "$candidate" ]; then
      exe="$candidate"
    fi
  done
  if [ -z "$exe" ]; then
    say "The session host downloads the preview browser (about 100 MB) the first time it draws a visual."
    return 0
  fi
  tmp="$(mktemp -d)"
  chown consort-session "$tmp"
  if command -v timeout >/dev/null 2>&1; then
    limit=(timeout 60)
  fi
  if out="$(as_user consort-session ${limit[@]+"${limit[@]}"} env HOME="$tmp" "$exe" --headless --disable-gpu --disable-dev-shm-usage \
    --user-data-dir="${tmp}/profile" --screenshot="${tmp}/test.png" --window-size=200,100 'data:text/html,test' 2>&1)" \
    && [ -s "${tmp}/test.png" ]; then
    rm -rf "$tmp"
    say "The preview browser starts."
    return 0
  fi
  rm -rf "$tmp"
  case "$out" in
    *"No usable sandbox"*)
      warn "the preview browser cannot make its sandbox, so a visual has no image. On Ubuntu 24.04 and later, AppArmor can stop it. See docs/deploy.md"
      ;;
    *"error while loading shared libraries"*)
      warn "the preview browser cannot start, because a library is missing, so a visual has no image. Install the libraries that docs/deploy.md lists, then run this script again. The error was: $(printf '%s\n' "$out" | grep 'shared libraries' | head -n 1)"
      ;;
    *)
      warn "the preview browser did not draw a test page, so a visual may have no image. The error was: $(printf '%s\n' "$out" | tail -n 1)"
      ;;
  esac
  return 0
}

# Pairs the session host with the hub. It signs the public URL of the hub, so
# it pairs with that address and not with another one.
paired=0
pair_session_host() {
  local toml="${HOST_DATA}/config/session-host.toml" url code
  if [ -s "$toml" ] && grep -q '^worker_id' "$toml"; then
    say "The session host is paired already."
    paired=1
    return 0
  fi
  url="$(current_public_url)"
  if ! wait_for "${url%/}/healthz" 90; then
    warn "${url} does not answer yet, so the session host is not paired. When it answers, run this script again."
    return 0
  fi
  code="$(as_user consort "$INSTALL_BIN" hub session-host-code --data "$HUB_DATA" | parse_setup_code)"
  [ -n "$code" ] || fail "the hub did not give a code for the session host. Run: journalctl -u consort-hub"
  as_user consort-session env HOME="$HOST_DATA" CONSORT_PAIRING_CODE="$code" \
    "$INSTALL_BIN" session-host pair "$url" --config-dir "${HOST_DATA}/config"
  paired=1
}

dry_run_text() {
  local key="$1" base="$2" asset="consort-${1}.tar.gz"
  say "Dry run. Nothing is downloaded or changed."
  say "System: ${key}"
  say "Install bubblewrap and git with apt or dnf, when they are missing."
  say "Install the libraries and fonts of the preview browser with apt or dnf, when they are missing."
  say "Download ${base}/${asset}"
  say "Check it against ${base}/SHA256SUMS"
  say "Create the users consort and consort-session, when they are missing."
  say "Put consort in ${INSTALL_BIN}"
  say "Install ${UNIT_DIR}/consort-hub.service and ${UNIT_DIR}/consort-session-host.service"
  if [ -n "$domain" ]; then
    say "Address: https://${domain}. Install Caddy, and write ${CADDY_DIR}/consort.caddy."
    say "Write CONSORT_PUBLIC_URL=https://${domain} in ${HUB_ENV}"
  else
    case "$address_mode" in
      tailscale)
        say "Address: Tailscale. Run: consort hub reach tailscale --data ${HUB_DATA}"
        say "Write CONSORT_PUBLIC_URL=${HUB_URL_LOCAL} in ${HUB_ENV}"
        ;;
      *) say "Address: the one that an earlier install set. This script keeps it." ;;
    esac
  fi
  say "Start consort-hub and consort-session-host."
  say "Pair the session host: consort hub session-host-code, then consort session-host pair"
  say "Check that bubblewrap can make a user namespace, and fix AppArmor when it can."
  say "Add an AppArmor profile for the preview browser, when AppArmor limits user namespaces."
  say "Print the address of the hub and a new setup code."
}

main() {
  local key base program_changed=0 old_version new_version
  orig_args=("$@")
  parse_args "$@"
  key="$(system_key)"
  base="$(release_base)"

  if [ "$dry_run" -eq 1 ]; then
    # Same address check as a real run, so a dry run says what would stop it.
    if [ -z "$domain" ] && ! { [ -e "${UNIT_DIR}/consort-hub.service" ] && [ -e "${HUB_DATA}/hub.db" ]; }; then
      if tailscale_ready; then address_mode="tailscale"; else explain_choices; exit 1; fi
    elif [ -z "$domain" ]; then
      address_mode="kept"
    else
      address_mode="domain"
    fi
    dry_run_text "$key" "$base"
    return 0
  fi

  require_root
  require_systemd
  choose_address
  command -v runuser >/dev/null 2>&1 || command -v sudo >/dev/null 2>&1 || fail "runuser or sudo must be installed"

  # Everything until the checksum is right happens in a temporary folder, so a
  # wrong checksum leaves the server as it was.
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT
  fetch_program "$work" "$key" "$base"

  if [ -x "$INSTALL_BIN" ]; then
    old_version="$("$INSTALL_BIN" --version 2>/dev/null | awk '{ print $2 }' || true)"
    new_version="$("${work}/consort" --version 2>/dev/null | awk '{ print $2 }' || true)"
    if [ -n "$old_version" ] && [ -n "$new_version" ] \
      && [ "$(printf '%s\n%s\n' "$old_version" "$new_version" | sort -V | head -n 1)" != "$old_version" ]; then
      fail "this server has version ${old_version}, which is newer than ${new_version}. An older program cannot read a newer database. Nothing was changed"
    fi
    cmp -s "${work}/consort" "$INSTALL_BIN" || program_changed=1
  else
    program_changed=1
  fi

  install_packages
  ensure_user consort "$HUB_DATA"
  ensure_user consort-session "$HOST_DATA"
  install -d -m 0700 -o consort -g consort "$HUB_DATA"
  install -d -m 0700 -o consort-session -g consort-session "$HOST_DATA"

  # Write to a new name and rename it, so a running service keeps its old file
  # until the rename, and a failed copy leaves the old file in place.
  if [ "$program_changed" -eq 1 ]; then
    cp "${work}/consort" "${INSTALL_BIN}.new"
    chmod 755 "${INSTALL_BIN}.new"
    mv -f "${INSTALL_BIN}.new" "$INSTALL_BIN"
    say "Installed ${INSTALL_BIN}"
  fi

  changed=0
  install_unit consort-hub.service unit_hub
  install_unit consort-session-host.service unit_session_host
  write_address_settings
  if [ "$changed" -eq 1 ]; then
    program_changed=1
  fi
  systemctl daemon-reload

  start_service consort-hub "$program_changed"
  wait_for "${HUB_URL_LOCAL}/healthz" 60 || fail "the hub did not start. Run: journalctl -u consort-hub"
  say "The hub runs."

  case "$address_mode" in
    tailscale) setup_tailscale ;;
    domain)
      if setup_caddy; then
        hub_address="https://${domain}"
        if ! wait_for "${hub_address}/healthz" 90; then
          warn "${hub_address} does not answer yet. Caddy may still be getting the certificate. Check the DNS record of ${domain} and that ports 80 and 443 are open. Run: journalctl -u caddy"
          address_failed=1
        fi
      else
        address_failed=1
      fi
      ;;
    kept) hub_address="$(current_public_url)" ;;
    *) ;;
  esac

  # The session host needs the AppArmor fix before it starts.
  check_boundary
  check_browser
  # The session host signs the public URL, so it can pair only when that
  # address answers.
  if [ "$address_failed" -eq 0 ]; then
    pair_session_host
  fi
  if [ "$paired" -eq 1 ]; then
    start_service consort-session-host "$program_changed"
  fi

  [ "$address_failed" -eq 0 ] || fail "the hub runs on this server, but it has no address that works. Fix the problem above, then run this script again. The data stays"

  local setup
  setup="$(as_user consort "$INSTALL_BIN" hub setup-code --data "$HUB_DATA" | parse_setup_code)"
  say ""
  say "Consort hub is installed."
  say ""
  say "  Hub address:  ${hub_address}"
  say "  Setup code:   ${setup}   (it works once and expires in 10 minutes)"
  say ""
  say "Open the Consort app, choose On a server, and enter the address and the code."
  say "For a new code, run: sudo -u consort consort hub setup-code --data ${HUB_DATA}"
  if [ "$paired" -eq 0 ]; then
    say ""
    say "The session host is not paired yet. Run this script again when the hub address answers."
  fi
  say "Sign in to a provider in the app. The session host runs on this server."
}

if [ -z "${CONSORT_HUB_SH_SOURCE_ONLY:-}" ]; then
  main "$@"
fi
