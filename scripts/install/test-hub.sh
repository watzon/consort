#!/usr/bin/env bash
# Tests hub.sh with no root and with no change to this machine. It checks:
#   - shellcheck passes, when shellcheck is installed;
#   - --help, --dry-run and the refusals;
#   - that the units and the Caddyfile inside the script equal the files in
#     deploy/;
#   - the checksum rule, with a fake release in a temporary folder;
#   - the choice of address, and the lines that the script writes in hub.env;
#   - the libraries of the preview browser: the names for Debian and Ubuntu (with
#     fake apt tools), and that the list is the same in the Dockerfile and in
#     docs/deploy.md;
#   - the AppArmor profile of the preview browser (with a fake apparmor_parser).
# It never installs a package, a user or a service. The real install has never
# run on a server: only a person on a Linux server can test that.
#
#   scripts/install/test-hub.sh
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "${here}/../.." && pwd)"
script="${here}/hub.sh"

root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT

pass() { echo "ok   $1"; }
die() { echo "FAIL $1" >&2; exit 1; }
check() { # check <description> <expected text> <actual text>
  case "$3" in
    *"$2"*) pass "$1" ;;
    *) die "$1: expected '$2' in: $3" ;;
  esac
}
absent() { # absent <description> <unexpected text> <actual text>
  case "$3" in
    *"$2"*) die "$1: found '$2' in: $3" ;;
    *) pass "$1" ;;
  esac
}

bash -n "$script" || die "hub.sh has a syntax error"
pass "hub.sh has no syntax error"
if command -v shellcheck >/dev/null 2>&1; then
  shellcheck "$script" "${here}/worker.sh" "$0" || die "shellcheck found a problem"
  pass "shellcheck is clean"
else
  echo "skip shellcheck is not installed"
fi

# A PATH with the basic tools and no Tailscale. A system directory may hold one,
# so build the folder by hand.
plain="$root/plain-bin"
mkdir -p "$plain"
for tool in awk basename cat chmod cmp cp cut dirname env grep head id install ls mkdir mktemp mv printf rm sed sleep sort tail tar tr uname wc date; do
  if path="$(command -v "$tool" 2>/dev/null)" && [ -x "$path" ]; then ln -sf "$path" "$plain/$tool"; fi
done
bash_path="$(command -v bash)"

with_dry() { # with_dry <path> args...: a dry run that pretends to be Linux
  local p="$1"
  shift
  PATH="$p" CONSORT_DRY_RUN_OS=Linux CONSORT_DRY_RUN_ARCH=x86_64 "$bash_path" "$script" --dry-run "$@"
}

out="$("$bash_path" "$script" --help 2>&1)"
check "--help shows the options" "--domain <name>" "$out"

out="$(with_dry "$plain" --domain Hub.Example.com 2>&1)"
check "a dry run says that nothing changes" "Nothing is downloaded or changed." "$out"
check "a dry run names the asset" "consort-linux-x86_64.tar.gz" "$out"
check "a dry run checks SHA256SUMS" "SHA256SUMS" "$out"
check "a dry run names the domain, in lower case" "Address: https://hub.example.com" "$out"
check "a dry run writes the public URL" "CONSORT_PUBLIC_URL=https://hub.example.com" "$out"
check "a dry run names the libraries of the preview browser" "libraries and fonts of the preview browser" "$out"
check "a dry run names the AppArmor profile of the preview browser" "AppArmor profile for the preview browser" "$out"

out="$(PATH="$plain" CONSORT_DRY_RUN_OS=Linux CONSORT_DRY_RUN_ARCH=aarch64 CONSORT_DOWNLOAD_BASE=file:///x "$bash_path" "$script" --dry-run --domain a.example.com 2>&1)"
check "a dry run for arm64 names the arm64 asset" "consort-linux-arm64.tar.gz" "$out"
check "a dry run reads CONSORT_DOWNLOAD_BASE" "file:///x/consort-linux-arm64.tar.gz" "$out"
out="$(with_dry "$plain" --domain a.example.com --version v0.2.0 2>&1)"
check "a dry run with --version names the tag" "releases/download/v0.2.0/" "$out"

# Without Tailscale and without --domain, the script stops and says why.
if out="$(with_dry "$plain" 2>&1)"; then die "no address must stop the script"; fi
check "no address names Tailscale" "Tailscale (recommended)" "$out"
check "no address names the domain way" "--domain hub.example.com" "$out"
absent "no address stops before any step" "Dry run." "$out"

# With a fake Tailscale that runs, a dry run uses it.
fake="$root/fake-bin"
mkdir -p "$fake"
cat >"$fake/tailscale" <<'FAKE'
#!/bin/sh
echo '{"BackendState": "Running"}'
FAKE
chmod +x "$fake/tailscale"
out="$(with_dry "$fake:$plain" 2>&1)"
check "a dry run uses Tailscale when it runs" "Address: Tailscale" "$out"
check "a dry run runs the reach command" "consort hub reach tailscale --data /var/lib/consort" "$out"
check "a dry run keeps the hub on loopback" "CONSORT_PUBLIC_URL=http://127.0.0.1:7600" "$out"
out="$(with_dry "$fake:$plain" --domain hub.example.com 2>&1)"
absent "--domain skips Tailscale" "reach tailscale" "$out"
cat >"$fake/tailscale" <<'FAKE'
#!/bin/sh
echo '{"BackendState": "Stopped"}'
FAKE
if out="$(with_dry "$fake:$plain" 2>&1)"; then die "a Tailscale that is not connected must stop the script"; fi
check "a Tailscale that is not connected is named" "is on this server, but it is not connected" "$out"

for bad in "https://hub.example.com" "hub" "hub.example.com/x" "bad_name.example.com" "-a.example.com" "a.example.com:8443"; do
  if with_dry "$plain" --domain "$bad" >/dev/null 2>&1; then die "the domain '$bad' must be refused"; fi
done
pass "bad domain names are refused"
if with_dry "$plain" --domain >/dev/null 2>&1; then die "--domain needs a value"; fi
pass "--domain needs a name"
if with_dry "$plain" --nope >/dev/null 2>&1; then die "an unknown option must fail"; fi
pass "an unknown option fails"

# Systems that the script does not support.
if [ "$(uname -s)" != Linux ]; then
  out="$("$bash_path" "$script" --domain hub.example.com 2>&1)" && die "a Mac must be refused"
  check "a Mac is sent to the app" "On a Mac, open the Consort app" "$out"
fi
if out="$(PATH="$plain" CONSORT_DRY_RUN_OS=Linux CONSORT_DRY_RUN_ARCH=riscv64 "$bash_path" "$script" --dry-run --domain a.example.com 2>&1)"; then
  die "an unknown processor must be refused"
fi
check "an unknown processor is named" "riscv64" "$out"
if out="$(PATH="$plain" CONSORT_DRY_RUN_OS=FreeBSD "$bash_path" "$script" --dry-run --domain a.example.com 2>&1)"; then
  die "FreeBSD must be refused"
fi
check "another system is named" "FreeBSD" "$out"
if [ "$(id -u)" -ne 0 ] && [ "$(uname -s)" = Linux ]; then
  # A user that is not root, with no sudo: the script says what to do.
  if out="$(PATH="$plain" "$bash_path" "$script" --domain hub.example.com 2>&1)"; then die "a user that is not root, with no sudo, must be refused"; fi
  check "no sudo is explained" "sudo is not installed" "$out"
  # With sudo, the script runs itself again. A fake sudo prints what it gets,
  # and a fake curl gives a stub script, so nothing runs for real.
  sudo_bin="$root/sudo-bin"
  mkdir -p "$sudo_bin"
  cat >"$sudo_bin/sudo" <<'FAKE'
#!/bin/sh
if [ "$2" = "-c" ]; then echo "SUDO: $1 $2 $3 $4 $5 $6"; else echo "SUDO: $1 $2 $3 $4"; fi
FAKE
  cat >"$sudo_bin/curl" <<'FAKE'
#!/bin/sh
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then echo "echo stub" >"$2"; fi
  shift
done
FAKE
  chmod +x "$sudo_bin/sudo" "$sudo_bin/curl"
  out="$(PATH="$sudo_bin:$plain" "$bash_path" -s -- --domain hub.example.com <"$script" 2>&1)" || die "the piped script must run itself again with sudo: $out"
  check "a piped script runs again through sudo" "SUDO: bash -c echo stub hub.sh --domain hub.example.com" "$out"
    out="$(PATH="$sudo_bin:$plain" "$bash_path" "$script" --domain hub.example.com 2>&1)" || die "a script from a file must run itself again with sudo: $out"
  check "a script from a file runs again through sudo" "SUDO: bash $script --domain hub.example.com" "$out"
fi

# From here the tests source the functions. Nothing runs.
export CONSORT_HUB_SH_SOURCE_ONLY=1
# shellcheck source=scripts/install/hub.sh
source "$script"
set +e

# The files inside the script equal the files in deploy/.
diff <(unit_hub) "$repo/deploy/hub/consort-hub.service" >/dev/null || die "the hub unit in hub.sh differs from deploy/hub/consort-hub.service"
pass "the hub unit in hub.sh equals deploy/hub/consort-hub.service"
diff <(unit_session_host) "$repo/deploy/session-host/consort-session-host.service" >/dev/null || die "the session host unit in hub.sh differs from deploy/session-host/consort-session-host.service"
pass "the session host unit in hub.sh equals deploy/session-host/consort-session-host.service"
diff <(caddy_template) "$repo/deploy/hub/Caddyfile" >/dev/null || die "the Caddyfile in hub.sh differs from deploy/hub/Caddyfile"
pass "the Caddyfile in hub.sh equals deploy/hub/Caddyfile"
site="$(caddy_site hub.example.org)"
check "the Caddy site uses the domain" "hub.example.org {" "$site"
absent "the Caddy site keeps no placeholder" "hub.example.com" "$site"
# hub.env overrides the placeholder in the unit, so the unit has to read it.
unit="$(unit_hub)"
check "the hub unit reads hub.env" "EnvironmentFile=-/etc/consort/hub.env" "$unit"
check "the hub unit sets the install kind" "CONSORT_INSTALL_KIND=service" "$unit"

# The parsers for the output of consort.
check "the setup code is read" "ABCD-EFGH" "$(printf 'Client setup code: ABCD-EFGH\nIt works once and is valid for 10 minutes.\n' | parse_setup_code)"
check "the session host code is read" "WXYZ-1234" "$(printf 'Session host code: WXYZ-1234\nIt works once.\n' | parse_setup_code)"
check "the reach address is read" "https://box.tail1234.ts.net:8443" "$(printf 'The hub answers at https://box.tail1234.ts.net:8443.\nThe hub accepts this address now. It needs no restart.\n' | parse_reach_address)"
same() { # same <description> <expected> <actual>
  if [ "$2" = "$3" ]; then pass "$1"; else die "$1: expected '$2', got '$3'"; fi
}
same "an unknown output gives no address" "" "$(printf 'nothing useful\n' | parse_reach_address)"

# The checksum rule, with a fake release.
rel="$root/release"
pkg="$root/package"
mkdir -p "$rel" "$pkg"
printf '#!/bin/sh\necho fake\n' >"$pkg/consort"
chmod +x "$pkg/consort"
tar -czf "$rel/consort-linux-x86_64.tar.gz" -C "$pkg" consort
echo "$(sha256_of "$rel/consort-linux-x86_64.tar.gz")  consort-linux-x86_64.tar.gz" >"$rel/SHA256SUMS"
work="$root/work-good"
mkdir -p "$work"
out="$( (fetch_program "$work" linux-x86_64 "file://$rel") 2>&1)" || die "a right checksum must pass: $out"
check "a right checksum passes" "Checksum is correct." "$out"
[ -x "$work/consort" ] || die "the program is not in the work folder"
pass "the checked program is in the work folder"

echo "deadbeef  consort-linux-x86_64.tar.gz" >"$rel/SHA256SUMS"
work="$root/work-bad"
mkdir -p "$work"
if out="$( (fetch_program "$work" linux-x86_64 "file://$rel") 2>&1)"; then die "a wrong checksum must stop"; fi
check "a wrong checksum is refused" "does not match" "$out"
check "a wrong checksum says that nothing was installed" "Nothing was installed" "$out"
[ ! -e "$work/consort" ] || die "a refused download left a program"
pass "a wrong checksum leaves no program"

echo "" >"$rel/SHA256SUMS"
work="$root/work-none"
mkdir -p "$work"
if out="$( (fetch_program "$work" linux-x86_64 "file://$rel") 2>&1)"; then die "a missing entry must stop"; fi
check "a missing checksum entry is refused" "has no entry" "$out"

if out="$( (fetch_program "$work" linux-x86_64 "file://$root/none") 2>&1)"; then die "a missing release must stop"; fi
check "a missing release is explained" "could not download" "$out"

# hub.env keeps other lines, and a second run changes nothing.
ENV_DIR="$root/etc"
HUB_ENV="$ENV_DIR/hub.env"
mkdir -p "$ENV_DIR"
printf 'RUST_LOG=debug\nCONSORT_PUBLIC_URL=https://old.example.com\n' >"$HUB_ENV"
changed=0
set_env_line CONSORT_PUBLIC_URL https://hub.example.com
[ "$changed" -eq 1 ] || die "a new value must count as a change"
env_text="$(cat "$HUB_ENV")"
check "hub.env keeps the other settings" "RUST_LOG=debug" "$env_text"
check "hub.env gets the new URL" "CONSORT_PUBLIC_URL=https://hub.example.com" "$env_text"
absent "hub.env drops the old URL" "old.example.com" "$env_text"
changed=0
set_env_line CONSORT_PUBLIC_URL https://hub.example.com
[ "$changed" -eq 0 ] || die "the same value must not count as a change"
pass "a second run changes nothing in hub.env"
set_env_line "${MARK%=}" "domain hub.example.com"
set_env_line "${MARK%=}" "tailscale"
[ "$(grep -c "^${MARK}" "$HUB_ENV")" -eq 1 ] || die "hub.env must have one mark"
pass "hub.env has one mark"
# A run that sets both lines again must not count as a change, or every run
# would restart the services.
changed=0
set_env_line CONSORT_PUBLIC_URL https://hub.example.com
set_env_line "${MARK%=}" "tailscale"
[ "$changed" -eq 0 ] || die "a second run with the same lines must not count as a change"
pass "two lines set again do not count as a change"
check "current_public_url reads hub.env" "https://hub.example.com" "$(current_public_url)"

# The choice of address on a second run.
UNIT_DIR="$root/units"
HUB_DATA="$root/data"
mkdir -p "$UNIT_DIR" "$HUB_DATA"
choose() { # choose <env file text or -> <installed 0|1> [domain]
  rm -f "$HUB_ENV" "$UNIT_DIR/consort-hub.service" "$HUB_DATA/hub.db"
  [ "$1" = "-" ] || printf '%s\n' "$1" >"$HUB_ENV"
  if [ "$2" = 1 ]; then : >"$UNIT_DIR/consort-hub.service"; : >"$HUB_DATA/hub.db"; fi
  domain="${3:-}"
  address_mode=""
  (choose_address >/dev/null 2>&1; echo "$address_mode")
}
same "a second run keeps Tailscale" tailscale "$(choose "${MARK}tailscale" 1)"
same "a second run keeps the domain" domain "$(choose "${MARK}domain hub.example.com" 1)"
same "an install by hand keeps its address" kept "$(choose "CONSORT_PUBLIC_URL=https://x.example.com" 1)"
same "--domain changes the address" domain "$(choose - 1 b.example.com)"
# A first run with no domain and with no Tailscale stops. The PATH has no
# Tailscale, so the answer is the empty mode.
saved_path="$PATH"
PATH="$plain"
same "a first run with no address and no Tailscale stops" "" "$(choose - 0)"
PATH="$saved_path"

# The libraries of the preview browser. A fake apt-cache, dpkg-query and apt-get
# stand for a Debian system.
fb="$root/fake-apt"
mkdir -p "$fb"
cat >"$fb/apt-cache" <<'FAKE'
#!/bin/sh
# Answers `apt-cache policy <name>`. libasound2 is virtual, as on Ubuntu 24.04.
case "$2" in
  libasound2) printf '%s\n' "libasound2:" "  Installed: (none)" "  Candidate: (none)" ;;
  libasound2t64) printf '%s\n' "libasound2t64:" "  Installed: (none)" "  Candidate: 1.2.11-1" ;;
  libnss3) printf '%s\n' "libnss3:" "  Installed: (none)" "  Candidate: 2:3.98-1" ;;
esac
FAKE
cat >"$fb/dpkg-query" <<'FAKE'
#!/bin/sh
# Prints "installed" for each name in FAKE_INSTALLED, and fails for the others.
for last; do :; done
case " $FAKE_INSTALLED " in
  *" $last "*) echo installed ;;
  *) exit 1 ;;
esac
FAKE
printf '#!/bin/sh\nexit 0\n' >"$fb/apt-get"
chmod +x "$fb/apt-cache" "$fb/dpkg-query" "$fb/apt-get"
saved_path="$PATH"
PATH="$fb:$saved_path"
same "a virtual package takes its t64 name" libasound2t64 "$(deb_pick libasound2)"
same "a real package keeps its name" libnss3 "$(deb_pick libnss3)"
same "an unknown package keeps its name" libnothing "$(deb_pick libnothing)"
inst=""
for n in "${BROWSER_DEB_PACKAGES[@]}"; do
  case "$n" in
    libasound2) inst="$inst libasound2t64" ;;
    libx11-6) ;;
    *) inst="$inst $n" ;;
  esac
done
FAKE_INSTALLED="$inst" find_missing_browser_packages
same "the t64 name counts as installed, and a missing one is found" "libx11-6" "${browser_missing[*]}"
FAKE_INSTALLED="$inst libx11-6" find_missing_browser_packages
same "no library is missing when all are installed" "0" "${#browser_missing[@]}"
FAKE_INSTALLED="" find_missing_browser_packages
same "every library is missing on a bare system" "${#BROWSER_DEB_PACKAGES[@]}" "${#browser_missing[@]}"
# A fake rpm and dnf stand for Fedora. The PATH has no apt-get.
fr="$root/fake-rpm"
mkdir -p "$fr"
cat >"$fr/rpm" <<'FAKE'
#!/bin/sh
# Answers `rpm -q --quiet --whatprovides <name>` for each name in FAKE_INSTALLED.
for last; do :; done
case " $FAKE_INSTALLED " in
  *" $last "*) exit 0 ;;
  *) exit 1 ;;
esac
FAKE
printf '#!/bin/sh\nexit 0\n' >"$fr/dnf"
chmod +x "$fr/rpm" "$fr/dnf"
PATH="$fr:$plain"
FAKE_INSTALLED="alsa-lib" find_missing_browser_packages
same "dnf finds each library that is missing" "$((${#BROWSER_RPM_PACKAGES[@]} - 1))" "${#browser_missing[@]}"
FAKE_INSTALLED="liberation-fonts" find_missing_browser_packages
absent "a name that a package provides counts as installed" "liberation-fonts" "${browser_missing[*]}"
PATH="$saved_path"
# One list in the script, in the Dockerfile and in the docs.
for n in "${BROWSER_DEB_PACKAGES[@]}"; do
  grep -qw -- "$n" "$repo/deploy/session-host/Dockerfile" || die "the Dockerfile of the session host does not install $n"
  grep -qw -- "$n" "$repo/docs/deploy.md" || die "docs/deploy.md does not name $n"
done
pass "the libraries of the preview browser are in the Dockerfile and in docs/deploy.md"

# The AppArmor profile of the preview browser.
APPARMOR_DIR="$root/apparmor"
HOST_DATA="$root/host"
USERNS_RESTRICT_FILE="$root/restrict"
mkdir -p "$APPARMOR_DIR" "$HOST_DATA"
text="$(userns_profile_text consort-chrome-headless-shell "$(browser_path_glob)")"
check "the profile names the browser in the tools folder" "profile consort-chrome-headless-shell ${HOST_DATA}/home/tools/chrome-headless-shell/*/chrome-headless-shell flags=(unconfined) {" "$text"
check "the profile has the userns rule" "  userns," "$text"
check "the profile has a local include" "include if exists <local/consort-chrome-headless-shell>" "$text"
absent "a system with no abi folder has no abi line" "abi <abi/4.0>" "$text"
mkdir -p "$APPARMOR_DIR/abi"
: >"$APPARMOR_DIR/abi/4.0"
check "a system with the abi file has the abi line" "abi <abi/4.0>," "$(userns_profile_text consort-bwrap /usr/bin/bwrap)"
check "the profile of bwrap names its path" "profile consort-bwrap /usr/bin/bwrap flags=(unconfined) {" "$(userns_profile_text consort-bwrap /usr/bin/bwrap)"
rm -rf "$APPARMOR_DIR/abi"

parser_log="$root/parser.log"
cat >"$fb/apparmor_parser" <<FAKE
#!/bin/sh
echo "\$*" >>"$parser_log"
FAKE
chmod +x "$fb/apparmor_parser"
chown() { :; }
as_user() {
  shift
  "$@"
}
profile_file="$APPARMOR_DIR/consort-chrome-headless-shell"

printf '1\n' >"$USERNS_RESTRICT_FILE"
out="$(PATH="$fb:$saved_path" check_browser 2>&1)"
[ -f "$profile_file" ] || die "AppArmor limits user namespaces, and check_browser wrote no profile"
check "the profile file names the browser" "${HOST_DATA}/home/tools/chrome-headless-shell/*/chrome-headless-shell" "$(cat "$profile_file")"
check "the profile is loaded" "-r $profile_file" "$(cat "$parser_log")"
check "a browser that is not there yet is explained" "downloads the preview browser" "$out"
PATH="$fb:$saved_path" check_browser >/dev/null 2>&1
same "a second run does not load the profile again" 1 "$(wc -l <"$parser_log" | tr -d ' ')"

rm -f "$profile_file" "$parser_log"
printf '0\n' >"$USERNS_RESTRICT_FILE"
PATH="$fb:$saved_path" check_browser >/dev/null 2>&1
[ ! -e "$profile_file" ] || die "no restriction must write no profile"
[ ! -e "$parser_log" ] || die "no restriction must not load a profile"
pass "no AppArmor restriction needs no profile"

printf '1\n' >"$USERNS_RESTRICT_FILE"
out="$(PATH="$plain" check_browser 2>&1)"
check "a restriction with no apparmor_parser is named" "apparmor_parser is not installed" "$out"
[ ! -e "$profile_file" ] || die "no apparmor_parser must write no profile"

# A browser that is there is run. A fake one stands for each result.
tool_dir="$HOST_DATA/home/tools/chrome-headless-shell/9.9.9"
mkdir -p "$tool_dir"
fake_browser() { # fake_browser <shell code of the program>
  printf '#!/bin/sh\n%s\n' "$1" >"$tool_dir/chrome-headless-shell"
  chmod +x "$tool_dir/chrome-headless-shell"
}
# shellcheck disable=SC2016
printed='for a; do case "$a" in --screenshot=*) printf png >"${a#--screenshot=}" ;; esac; done'
printf '0\n' >"$USERNS_RESTRICT_FILE"
fake_browser "$printed"
out="$(PATH="$fb:$saved_path" check_browser 2>&1)"
check "a browser that draws the test page is reported" "The preview browser starts." "$out"
fake_browser 'echo "[1:FATAL:zygote_host_impl_linux.cc:130] No usable sandbox! If you are running" >&2; exit 1'
out="$(PATH="$fb:$saved_path" check_browser 2>&1)"
check "a browser with no sandbox is named" "cannot make its sandbox" "$out"
check "a browser with no sandbox points at the docs" "docs/deploy.md" "$out"
fake_browser 'echo "chrome-headless-shell: error while loading shared libraries: libnss3.so: cannot open shared object file" >&2; exit 127'
out="$(PATH="$fb:$saved_path" check_browser 2>&1)"
check "a missing library is named" "a library is missing" "$out"
check "a missing library says which one" "libnss3.so" "$out"
fake_browser 'exit 0'
out="$(PATH="$fb:$saved_path" check_browser 2>&1)"
check "a browser that draws nothing is named" "did not draw a test page" "$out"
unset -f chown as_user

echo "All hub installer tests passed."
