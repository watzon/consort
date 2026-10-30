# Phase 5, part a: install and deploy

These notes are for the merge into `docs/DECISIONS.md`. The plan is [FINISH-PLAN.md](../FINISH-PLAN.md), phase 5, items 1 to 3 (the persistent client cache and the Windows checks on bigboi are other work). The user guides are [install.md](../install.md) and [deploy.md](../deploy.md). Written on 2026-10-30.

## What was built

### Worker service (`crates/consort-cli`)

- `consort worker install --hub <url> [--code <code>] [--name] [--tag] [--config-dir] [--exe] [--dry-run] [--linger]`, `consort worker uninstall` and `consort worker status`. Install pairs the machine with the same code path as `consort worker pair` (`consort_worker::pair`), then installs a per-user service. A machine that is paired with the same hub already needs no code, so a second install only rewrites the service.
- `service.rs` has no I/O in its first layer. `plan_install`, `plan_uninstall` and `plan_status` take plain strings (`ServiceEnv`) and return a `Plan`: folders, files with their text, files to remove, and commands. The text of the launchd plist, the systemd unit and the Task Scheduler XML is made in code. The tests check the text for all three systems on every system (the Windows paths are strings, not `PathBuf`). `apply` writes the files and runs the commands through a `Runner`. The tests use a recording runner, so no test touches `launchctl`, `systemctl` or `schtasks`.
- `--dry-run` prints the files with their text and the commands, does not pair, does not use the code, and writes nothing. It was run on this Mac for install, uninstall and status.
- `consort worker run --log-file <path>` writes the log to a file (append, no colors). A log over 10 MiB moves to `<file>.1` at the start. The services use it, so one flag works on all three systems. The log files are in [install.md](../install.md).
- `consort hub` reads `CONSORT_HUB_DATA`, `CONSORT_HUB_LISTEN` and `CONSORT_PUBLIC_URL` (the Docker image and the systemd unit use them).
- No service was installed or loaded on this machine, and no `launchctl`, `systemctl` or `schtasks` command ran.

### Install scripts (`scripts/install/`)

- `worker.sh` (macOS and Linux) and `worker.ps1` (Windows). They download `consort-<key>.tar.gz` (`.zip` on Windows) from the latest release, check the SHA-256 against `SHA256SUMS`, put `consort` on the PATH of the user, and run `consort worker install` with the code in `CONSORT_PAIRING_CODE`.
- `bash -n` and `shellcheck` pass for every shell script in `scripts/`. `scripts/install/test-worker.sh` runs `worker.sh` against a fake release in a temporary folder, with a fake `consort` (11 checks: checksum, PATH line once, arguments, a wrong checksum leaves nothing, and more). `worker.ps1` is not linted or run: this machine has no PowerShell. Run it on bigboi.

### Hub deployment (`deploy/hub/`, `Dockerfile`, `docs/deploy.md`)

- `consort-hub.service`: a hardened systemd unit (a system user, `StateDirectory`, `ProtectSystem=strict`, no capabilities, a system call filter, and more). It listens on `127.0.0.1:7600`. `systemd-analyze` is not on this machine, so the unit was not checked by systemd.
- `Dockerfile` and `docker-compose.yml` with a Caddy service. The image was built and run on this machine with OrbStack: `/healthz` answered, the hub ran as uid 10001 and the health check was starting. The Caddyfiles pass `caddy validate`.
- `Caddyfile` for a hub that runs as a service. WebSocket needs no setting in Caddy. `docs/deploy.md` covers the install, TLS, Docker, backups, upgrades and a check list.

### Release builds (`.github/workflows/release.yml`, `scripts/bundle-macos.sh`)

- The workflow builds `consort` for five targets, the macOS app (arm64 and x86_64, zip and dmg), the Windows desktop zip and the Linux desktop tarball. It writes `SHA256SUMS` and attaches everything to the release for the tag. The tag must match the version in `Cargo.toml`. `actionlint` passes. The workflow did not run: the repo has no remote.
- `scripts/bundle-macos.sh` builds `Consort.app` (bundle ID `dev.consort.desktop`, the version from `Cargo.toml`, minimum macOS 12, the icon), signs it (ad hoc, or with `APPLE_SIGNING_IDENTITY`), zips it with `ditto`, makes a dmg with `--dmg`, and notarizes when the Apple secrets exist. Tested here: the release build of this branch became `dist/Consort.app`, the zip and the dmg. The app started with `--mock` through `open -n`, one screenshot was taken (`docs/screenshots/phase5-bundled-app-dark.png`), and the app was stopped by its PID.
- The app icon was the Three voices mark already (`design/mark/generate.mjs` writes `design/logo/app-icon-*.svg`), so no new icon design was needed. `scripts/make-icons.sh` renders it with `rsvg-convert` (the SVG filter and the screen blend need a full renderer), makes the PNG sizes and `Consort.icns` with `sips` and `iconutil`, and puts the tile at 80 percent of the canvas, as the macOS icon grid asks. The output is in `design/logo/icons/`.

### App updates (`crates/consort-client/src/update/`, `ui/update_notice.rs`)

- `mod.rs` (pure): `Version` with compare, the platform key and the file name of the desktop file, `choose_update`, the `SHA256SUMS` parser, the `UpdateState` machine with `UpdateEvent`, `UpdateView` (with `notice()` and `summary()` for About) and `UpdateSettings`. `net.rs`: the GitHub call, the download into `<cache>/consort/updates/<version>/` with a hash check, and the background service (`Updates`: a first check 15 s after start, then every 6 hours, `check_now`, `download_now`, `set_auto`, `install_and_relaunch`). `apply.rs`: the replacement of the app.
- Tests: 41 in the client crate. The network code runs against a local fake server (axum): a newer release downloads, a wrong hash is deleted and never ready, the same version is up to date, a repo with no release, a 503, an unreachable server, the switch off (waits for `download_now`), the switch on again (downloads, and the switch is on disk), a staged file is not downloaded again, a cut file is, and old versions are removed. The macOS replace runs for real on temporary folders with a zip made by `ditto`: it swaps the bundles, refuses an unsigned update, refuses a zip without the app, and keeps the old app when the folder cannot be written. The Windows and Linux scripts are checked as text.
- The desktop shows the notice above the account row (`ConsortApp::update_notice`), redraws when the state changes (`follow_updates`), and quits after a good `install_and_relaunch`. `ConsortApp::updates` is the handle that About reads.

### Add worker dialog (`ui/add_worker.rs`)

- Three steps, as in Paper `E3F-0` and `EI4-0`. It replaces the old dialog and its text "There is no installer yet". The commands come from `consort_client::update::worker_install_command`. The first step label turns into the system after step 1. The dialog moves to step 3 by itself when the worker that it paired is online.
- Screens: `--mock --screen add-worker`, `add-worker-install`, `worker-added` (step 3) and `update-ready`. Screenshots: `docs/screenshots/phase5-add-worker-system-dark.png`, `phase5-add-worker-install-dark.png`, `phase5-add-worker-connect-dark.png`, `phase5-update-notice-dark.png`. They match the artboards, except for the choices below.
- 5 headless window tests in `phase5_tests.rs` and 6 unit tests in `add_worker.rs`.

## Decisions

| # | Area | Decision | Why | Easy to change? |
|---|---|---|---|---|
| P5A-1 | Install | Installs come from GitHub Releases, and the scripts run from `raw.githubusercontent.com/<repo>/main/scripts/install/`. `get.consort.dev` is not used anywhere. The repo name is one constant per language: `RELEASES_REPO` (Rust), `REPO` (`worker.sh`), `$Repo` (`worker.ps1`). The workflow uses `github.repository`. | The user approved GitHub Releases until there is a domain. One constant is easy to change | Yes |
| P5A-2 | Install | The Windows command is `& ([scriptblock]::Create((irm <url>))) -Hub <hub> -Code <code>`, not `irm <url> \| iex; consort worker install ...`. The macOS and Linux command is `curl -fsSL <url> \| bash -s -- --hub <hub> --code <code>`. | `iex` cannot pass parameters to a script, and a second command would not find `consort` on the PATH until a new terminal | Yes |
| P5A-3 | Install | Release files have no version in their names (`consort-macos-arm64.tar.gz`, `Consort-macos-arm64.zip`) and one `SHA256SUMS` lists all of them. The platform key is `macos-arm64`, `macos-x86_64`, `linux-x86_64`, `linux-arm64` or `windows-x86_64`. | `releases/latest/download/<name>` then works without the version, and the scripts and the updater share one rule | Hard once users have scripts. Change both sides together |
| P5A-4 | Service | The service restarts the worker after a failure, not after a clean exit (`KeepAlive/SuccessfulExit=false`, `Restart=on-failure`, `RestartOnFailure`). | A stop that the user asks for must stay stopped | Yes |
| P5A-5 | Service | Every service starts `consort worker run --log-file <path>`. The worker opens and rotates the file. launchd and systemd also send stdout and stderr to the same file. | One mechanism on three systems. Task Scheduler cannot redirect output without a shell | Yes |
| P5A-6 | Service | The Windows task runs `powershell.exe -WindowStyle Hidden -Command "& 'consort.exe' worker run ..."; exit $LASTEXITCODE`. The task file is UTF-16 with a byte order mark. | `consort.exe` is a console program, so a direct start shows a window at each sign-in. The exit code decides the restart. `schtasks /XML` wants UTF-16 | Yes. Not run on Windows |
| P5A-7 | Service | The service gets the `PATH` of the shell at install time. | launchd and systemd start programs with a short `PATH`, and the harnesses need the user's tools | Yes |
| P5A-8 | Service | The launchd label is `dev.consort.worker` and the bundle ID is `dev.consort.desktop`. | A reverse-DNS name is needed, and the project has no domain. `dev.consort` follows the placeholder `consort.dev` | Hard after release: users have the label, and macOS keys the app's settings by the bundle ID. Choose the final domain before the first release |
| P5A-9 | Service | `--linger` runs `loginctl enable-linger` on request. The install does not do it by itself, and prints a hint on Linux. | It may ask for a password or change a system setting | Yes |
| P5A-10 | Hub | TLS stays with a documented reverse proxy (Caddy). No built-in ACME. | The plan allows it, and Caddy already does renewals, rate limits and the port 80 challenge. A built-in client would add certificate keys, renewals and port 80 to the hub. It can be added later with no change for proxy users | Yes |
| P5A-11 | Hub | `consort hub` reads `CONSORT_HUB_DATA`, `CONSORT_HUB_LISTEN` and `CONSORT_PUBLIC_URL`. | The systemd unit and the container need settings without arguments | Yes |
| P5A-12 | Hub | The Docker image runs as uid 10001 with `git` and `curl`, a volume at `/data`, and a health check on `/healthz`. The Compose file makes the hub filesystem read-only. | The hub needs `git` for skill sources. A health check lets Docker restart a stuck hub | Yes |
| P5A-13 | Release | The tag must match the version in `Cargo.toml`, or the workflow stops. A tag with `-` is a pre-release, which the updater and the scripts ignore. | The updater compares the tag with the version of the running app | Yes |
| P5A-14 | Release | The Windows and Linux desktop jobs have `continue-on-error`. The release needs the CLI and the macOS app only. | Nobody could run these two builds here, and a GPUI build on Linux needs system libraries that were chosen without a test. A failure there must not block the other files | Yes. Remove the flag when they work |
| P5A-15 | Release | The macOS app is also a `.zip` (the updater uses it) and a `.dmg` (people use it). Windows is a zip with one folder, and Linux a tarball. No AppImage and no installer. | `ditto` keeps what a bundle needs, and a zip needs no mount. An AppImage needs extra tools and FUSE, which this CI has not been tried with | Yes |
| P5A-16 | Release | The icon tile is 80 percent of the canvas in `Consort.icns` and full size in the PNG files. | The macOS icon grid. A full-size tile looks too large in the Dock | Yes |
| P5A-17 | Updates | The check runs 15 seconds after the start and every 6 hours. With the switch off, the app still checks, and it keeps the version as "Available" for About. It downloads only on `download_now`, or when the switch goes on. | The plan asks for a check at start and every 6 hours. A check costs one small request, and About can then say that a version exists | Yes |
| P5A-18 | Updates | The switch is the file `updates.json` in the client config folder, not a field in another settings file. | Other builds change the client settings at the same time. A separate file cannot conflict. The orchestrator can fold it into one settings file later | Yes |
| P5A-19 | Updates | The macOS app replaces itself with two renames next to the installed bundle, after `codesign --verify` of the new bundle, with a rollback. Windows and Linux use a script that waits for the process, copies the files and starts the app. | The plan asks for an atomic replace on macOS and a documented best effort elsewhere. Rust has no safe call for `renamex_np`, and the code forbids `unsafe` | Yes |
| P5A-20 | Updates | A build that runs from a `target/` folder, and the mock mode, never check and cannot install. | A dev build must not replace itself or download a release | Yes |
| P5A-21 | Updates | The update logic is in `consort-client`, not in the desktop crate. | The desktop never talks to the network (AGENTS.md), and the mobile app can reuse the version, state and checksum code | Yes |
| P5A-22 | Dialog | The step 1 footer says "The worker starts when you sign in." The design said "The worker starts with the machine and updates itself." | The service starts at sign-in, and the worker does not update its own program | Yes |
| P5A-23 | Dialog | The online card shows the worker's own status line ("Online. Windows, x64, worker 0.4.1. Connected since 8:02 AM.") and not the design's "Windows 11, ... It connected at 7:52 PM." | The client has no OS version or connect time as separate fields. The status line is real data | Yes |
| P5A-24 | Dialog | The system choice starts at the system of this computer. | Most people add the machine they sit at | Yes |
| P5A-25 | Dialog | The third checklist line names the managed tools that are ready, and the number of accounts. With no tool ready it says that the worker downloads Claude Code and Codex when an account is added. | The design's "Found Claude Code and Codex" is true only when the tools are there | Yes |

## Rows for DECISIONS.md

| Area | Decision | Reason |
|---|---|---|
| Install | Installs come from GitHub Releases with scripts in `scripts/install/`. The repo name is one constant per language. `get.consort.dev` is not used. Release file names have no version, and `SHA256SUMS` lists all files. | Approved: GitHub Releases until there is a domain |
| CLI | `consort worker install`, `uninstall` and `status` pair the machine and manage a per-user service (launchd, systemd user unit, Task Scheduler). `--dry-run` prints the plan. `consort worker run --log-file` writes the log to a file. | Phase 5, item 2 |
| CLI | The service text is made from templates in code and tested on every system. The install runs commands through a `Runner`, so tests never touch a service manager. | Review of an install without an install |
| Hub | TLS is a documented Caddy reverse proxy. The hub unit is hardened. The Dockerfile and Compose file exist. The hub reads `CONSORT_HUB_DATA`, `CONSORT_HUB_LISTEN` and `CONSORT_PUBLIC_URL`. | Phase 5, item 3 |
| Release | The workflow builds the CLI for five targets and the desktop app for macOS, Windows and Linux. Windows and Linux desktop builds are best effort. `scripts/bundle-macos.sh` makes the app. | Phase 5, item 1 |
| Desktop | App updates: check at start and every 6 hours, a hash-checked download into a staging folder, "Update ready. Restart to install." The logic is in `consort-client`. The switch is `updates.json`. | Approved: on by default |
| Desktop | The Add worker dialog has three steps and replaces decision 122 ("There is no installer yet"). | Phase 5, item 2 |

## Checks

- `scripts/check.sh` (the whole workspace): see the last lines at the end of this file.
- `shellcheck` and `bash -n` on every script in `scripts/` and `scripts/install/`. `actionlint` on the workflow. `caddy validate` on both Caddyfiles. `docker compose config` on the Compose file.
- `scripts/install/test-worker.sh`: 11 checks pass.
- `cargo check --target x86_64-pc-windows-msvc -p consort-cli` did not finish: `libsqlite3-sys` (rusqlite, bundled) builds C code, and the C compiler of this Mac cannot find the Windows headers. The Windows service text is covered by unit tests that run on every system. The code of this build has no `cfg(windows)` block except in `consort-client/src/update/apply.rs` (`creation_flags`) and `consort-desktop/src/main.rs` (the window subsystem), and no part of it was compiled for Windows. Check them on bigboi.

## What was not done, and why

- **Nothing was installed.** No service was installed or loaded here, and no real GitHub release exists (the repo has no remote), so the install scripts ran only against a fake release, and the updater only against a fake server. The first real run is the test that is left.
- **Signing and notarization** need the Apple Developer ID. The scripts and the workflow are ready and run when the six secrets exist. The ad-hoc path was tested.
- **Windows.** The PowerShell script, the task XML, the Windows update script and the Windows desktop build were not run. The `.exe` has no icon (it needs a `.ico` and a build script).
- **The Linux desktop build in CI** uses a list of system libraries that was not tried. The job may need a change at the first run. It does not block the release.
- **An AppImage** and a Windows installer. The zip and the tarball stand in.
- **The persistent client cache and the Windows checks on bigboi** (phase 5, items 4 and 5) are not in this build.
- **The About dialog** is not built. `ConsortApp::updates` gives it `view()`, `check_now()`, `download_now()` and `set_auto()`. No Settings page has the switch yet. It is in `updates.json` and in `Updates::set_auto`.

## Last lines of the full `scripts/check.sh`

```
== Format
== Lint
== Tests
     Summary [  26.432s] 1541 tests run: 1541 passed, 25 skipped
== Doc tests

All checks passed in 85s.
```
