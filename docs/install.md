# Install Consort

This page shows how to install the desktop app and the worker, how a hub on this computer works, how the app updates itself, and how to make a release. To run a hub on a server, see [deploy.md](deploy.md): `scripts/install/hub.sh` installs it with one command.

Installs come from GitHub Releases on the Consort repository. Until Consort has its own domain, nothing else serves the files. The scripts are in the repo and run from `raw.githubusercontent.com`. The design placeholder `get.consort.dev` is not used.

## The repository name

The name of the repository is one constant in each of these places. Change all five when the repository moves.

| Where | What |
|---|---|
| `crates/consort-proto/src/release.rs` | `RELEASES_REPO`. The app, its updater, the install commands in the Add worker dialog, the hub command on the server setup screen and in the Move to a server dialog, `consort hub update` and the update of a worker use it. It is in `consort-proto` because the worker must not depend on `consort-client` |
| `scripts/install/worker.sh` | `REPO` |
| `scripts/install/hub.sh` | `REPO` |
| `scripts/install/worker.ps1` | `$Repo` |
| `.github/workflows/release.yml` | Nothing. The workflow uses `github.repository` |

The comments and the examples in the scripts and in these docs also show the name. They are text only.

## Install a worker

A worker is a machine where work runs. It has no account and no agent CLI. The limits of a worker (the highest approval mode, the folders and shell access) change in Settings > Workers, with the worker online. The Add worker dialog (Settings > Workers > Add worker) shows one command for each system. It has the pairing code in it.

macOS and Linux:

```
curl -fsSL https://raw.githubusercontent.com/watzon/consort/main/scripts/install/worker.sh | bash -s -- --hub https://hub.example.com --code ABCD-EFGH
```

Windows, in PowerShell:

```
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/watzon/consort/main/scripts/install/worker.ps1))) -Hub https://hub.example.com -Code ABCD-EFGH
```

The design showed `irm ... | iex; consort worker install ...`. The script takes its settings as parameters, so the scriptblock form replaces it. It also avoids a second command that needs the new PATH.

The script:

1. Finds the file for this system in the latest release: `consort-<system>-<processor>.tar.gz`, or `.zip` on Windows. The names are `macos-arm64`, `macos-x86_64`, `linux-x86_64`, `linux-arm64` and `windows-x86_64`.
2. Downloads it and the `SHA256SUMS` file of the release, and checks the SHA-256. A wrong checksum stops the install and leaves nothing behind.
3. Puts `consort` in `~/.local/bin` (macOS and Linux) or `%LOCALAPPDATA%\Consort\bin` (Windows), and adds that folder to your PATH.
4. Runs `consort worker install --hub <url>` with the code in the environment variable `CONSORT_PAIRING_CODE`, so other users of the machine do not see it in the process list.

For a hub on This network, the Add worker dialog adds `--tls-sha256 <hex>` (`-TlsSha256 <hex>`). It is the pin of the certificate of the hub, and the worker accepts no other certificate.

Options of both scripts: a worker name, tags, a release version, `--no-service` (`-NoService`) to install the program only, and `--dry-run` (`-DryRun`) to print the steps. Run `worker.sh --help` for the list. The variables `CONSORT_INSTALL_DIR` and `CONSORT_DOWNLOAD_BASE` change the folder and the download address.

### `consort worker install`

The command pairs the machine with the hub (with the same code path as `consort worker pair`) and installs a service for your user. It needs no admin rights.

```
consort worker install --hub https://hub.example.com --code ABCD-EFGH
consort worker status
consort worker uninstall
```

| System | Service | File |
|---|---|---|
| macOS | A launchd agent, label `dev.consort.worker` | `~/Library/LaunchAgents/dev.consort.worker.plist` |
| Linux | A systemd user unit | `~/.config/systemd/user/consort-worker.service` |
| Windows | A Task Scheduler task, `Consort worker` | `%LOCALAPPDATA%\Consort\worker-task.xml` (the task definition) |

The worker starts when you sign in, and starts again after a failure (an exit with an error, or a crash). A stop that you ask for stays stopped. On Linux, add `--linger` so that the worker also starts at boot, with no sign-in. It runs `loginctl enable-linger`.

The service starts the worker with the `PATH` that your shell had at install time, because launchd and systemd start programs with a short `PATH`. Run `consort worker install` again after you install a tool that the worker must find. A second install replaces the first one.

**Logs.** The worker writes its log to a file. A log larger than 10 MiB moves to `worker.log.1` when the worker starts.

| System | Log file |
|---|---|
| macOS | `~/Library/Logs/Consort/worker.log` |
| Linux | `$XDG_STATE_HOME/consort/worker.log`, by default `~/.local/state/consort/worker.log` |
| Windows | `%LOCALAPPDATA%\Consort\logs\worker.log` |

**See before you install.** `consort worker install --dry-run` prints the files and the commands and changes nothing. It does not use the pairing code.

**Remove.** `consort worker uninstall` stops the service and removes it. It keeps the worker folder (with the key and the accounts) and the log. Delete them by hand to remove all traces. To remove the program too, delete the `consort` file that the install script made.

**Update the worker.** A worker follows its hub by itself. When it connects to a newer hub, the hub names its version with `worker.update`. A hub that refuses the worker on the protocol names it in the error. The worker downloads that release from `RELEASES_REPO`, and checks it against `SHA256SUMS`. It waits until no process runs and no request is open, and then replaces its program in one step and restarts. It never moves to an older version. A failed update tries again after one hour. Settings > Workers shows where the update is. A worker in a cargo `target` folder or an app bundle never replaces itself, and neither does a debug build or `consort dev`. The folder of the program must be writable by the worker, otherwise Settings > Workers shows the error.

To update by hand, run the install script again with a new code, or with the same machine paired already: `consort worker install --hub <url>` with no code only rewrites the service. The script replaces `consort` in place and the service starts the new file at the next start. Restart it with `launchctl kickstart -k gui/$(id -u)/dev.consort.worker`, `systemctl --user restart consort-worker`, or `schtasks /End` and `/Run` for the task `Consort worker`.

## A hub on this computer

On a Mac or on Linux, the app can run the hub on the same computer. One service, for your user and with no admin rights, runs the hub, the session host as its child, and a worker. Windows has no local path, because Consort cannot keep an agent inside its limits there. `consort local check` says what this computer can do.

The app carries the `consort` program: `Consort.app/Contents/Helpers/consort` in the macOS app, and `consort` beside the app in the Linux archive. On a Mac, the file cannot be in `Contents/MacOS`, because `consort` and `Consort` are one file on a normal Mac disk. A debug build finds the program beside its binary. The Windows archive has the program too, but no local path uses it.

| Command | What it does |
|---|---|
| `consort local install` | Copies the program, adds the service, waits for a healthy hub, and prints JSON with the address, a setup code and the worker ID. It does this safely a second time. `--dry-run` prints the plan. It never writes the code to a log |
| `consort local status` | Prints JSON: the service, the address, the versions, the connected parts and the tasks that work |
| `consort local setup-code` | Prints a new code from the data folder, so the app connects with no typed code |
| `consort local start`, `stop` | Starts or stops the service. The service stays installed |
| `consort local login on`, `off` | Turns the start at sign-in on or off. A running hub keeps running |
| `consort local upgrade --program <file>` | Backs up, stops, replaces the program, starts, and puts everything back on a failure |
| `consort local uninstall` | Stops and removes the service and the program. It keeps the data, unless you add `--delete-data` |
| `consort local check` | Prints JSON that says whether this computer can run agents |
| `consort local run` | What the service runs. Do not run it yourself |

### Files

All files are in the Consort folder: `~/Library/Application Support/Consort` on macOS, and `~/.local/share/Consort` (the XDG data folder) on Linux. On macOS this folder also holds the settings of the app, so the hub keeps to these two folders, and an uninstall touches nothing else:

- `bin/consort`, and `bin/consort.previous` for a rollback.
- `local/state.json`: the address, the port and the version. Only your user can read it.
- `local/hub`, `local/session-host` and `local/worker`: the data of the three parts.
- `local/home`: the account profiles and the managed tools. The session host downloads the preview browser (about 125 MB) here the first time that it renders a visual. On Linux, the browser needs some system libraries. See "The preview browser" in [deploy.md](deploy.md).
- `local/update`: the program of an update, while the app replaces the copy.
- `local/backups` and `local/logs/local.log`.

The service is `dev.consort.local`. On macOS, launchd loads it from `local/service.plist`, and a copy in `~/Library/LaunchAgents` is the start at sign-in. On Linux it is the systemd user unit `dev.consort.local.service`. To check it: `launchctl print gui/$(id -u)/dev.consort.local` or `systemctl --user status dev.consort.local`.

`CONSORT_LOCAL_DIR` replaces the Consort folder and `CONSORT_LOCAL_LABEL` replaces the service name. Tests use them, and so does `scripts/dev.sh`, so a dev app never touches your real hub.

### The port

`consort local run` listens on a free port on `127.0.0.1`. It keeps the port across restarts while the port is free. If another program took it, the hub takes a new one, and the session host, the worker and the app follow with their keys. `state.json` keeps the old addresses, so the app moves only when its address was this hub's. A Tailscale Serve entry still points at the old port: Settings > Hub shows that reach is off, and you turn it on again.

### Updates of a local hub

A local hub updates with the app, and `consort hub update` refuses it. When you select Restart, the app does these steps before it replaces itself:

1. It reads the new program from the update file. A release without one skips the next steps.
1. If tasks work, the app asks: Restart when they finish, or Restart now. A restart of the service stops the session host and its agents. Close the dialog to do nothing.
1. The installed program makes a backup, stops the service, replaces the program, starts the service and waits for the new version to answer. The old program does this, because a newer program must not change a database that an older hub still uses.
1. If the new hub does not answer or a migration fails, the program puts back the previous program and the backup, starts the old hub, and the app says what happened. The app stays on its version.

The app does the same at each start when its program is newer than the installed copy. It never replaces a newer copy: then the app is too old, and it says so.

## Install the desktop app

Download the file for your system from the latest release:

| System | File | How |
|---|---|---|
| macOS | `Consort-macos-arm64.dmg` or `Consort-macos-x86_64.dmg` (the zip has the same app) | Open the dmg and drag Consort to Applications |
| Windows | `Consort-desktop-windows-x86_64.zip` | Unpack it and keep the `Consort` folder where you like. Run `consort-desktop.exe` |
| Linux | `Consort-desktop-linux-x86_64.tar.gz` | Unpack it. Run `consort-desktop`. `consort.desktop` and `consort.png` are for your menu |

Until the app is signed with a Developer ID and notarized, macOS blocks the first start: go to System Settings > Privacy & Security and select Open Anyway. A release that has the signing secrets (below) is signed and notarized, and starts with no question. The Windows and Linux files are not signed.

The app has the `com.apple.security.device.audio-input` entitlement and `NSMicrophoneUsageDescription`, because it captures the microphone in its own process.

The Windows and Linux desktop builds are best effort. The release workflow builds them, but a failed build does not stop the release.

## Updates

The app looks for a new release when it starts, and every 6 hours. It reads the latest release from the GitHub API, and chooses the file for its system and processor. With the switch "Get updates on their own" on (the default), it downloads the file in the background into a staging folder, and checks its SHA-256 against `SHA256SUMS`. A file with a wrong hash is deleted and never used. When the file is ready, the sidebar shows "Update ready. Restart to install." with Restart. The app also reads `release.json`. If that version cannot talk to the connected hub because the hub is too old, the update waits: the sidebar says so and offers Update the hub, and Restart is off until the hub is new enough. A release with no `release.json` holds nothing back.

Restart replaces the app and starts the new copy:

- **macOS.** The app unpacks the zip next to the installed `Consort.app`, checks the signature of the new bundle, and swaps the two bundles with two renames. If the second rename fails, the first one is undone. The app needs write access to the folder that holds `Consort.app`. If it has none, it says so and keeps the old app.
- **Windows and Linux.** A running program cannot replace itself. The app writes a small script, starts it, and quits. The script waits until the app has ended, copies the new files over the install folder, and starts the app again. This is a best effort: the install folder must be writable, and the script writes errors to `update.log` in the staging folder. If an update fails, install the release by hand.

A copy of the app that runs from a `cargo` build folder never checks and never replaces itself.

The switch is in `updates.json` in the config folder of the client (`~/Library/Application Support/consort` on macOS, `~/.config/consort` on Linux, `%APPDATA%\consort` on Windows, or `CONSORT_CONFIG_DIR`). With the switch off, the app still checks, but it does not download. The About dialog can show the state and a Download button. See `consort_client::update`.

The staging folder is `consort/updates` in the cache folder of the system. The app keeps only the newest version there.

## Make a release

1. Set `version` in `[workspace.package]` of `Cargo.toml`, and commit.
1. Run the full CI on that commit and wait for it to pass: `gh workflow run ci.yml`. A push runs only the fast checks.
1. Tag it and push the tag: `git tag v0.2.0 && git push origin v0.2.0`. The tag must match the version, or the workflow stops.
1. The workflow `.github/workflows/release.yml` builds everything and publishes the release:

| File | What |
|---|---|
| `consort-macos-arm64.tar.gz`, `consort-macos-x86_64.tar.gz`, `consort-linux-x86_64.tar.gz`, `consort-linux-arm64.tar.gz`, `consort-windows-x86_64.zip` | The `consort` program (CLI, hub and worker) |
| `Consort-macos-arm64.zip`, `Consort-macos-arm64.dmg`, `Consort-macos-x86_64.zip`, `Consort-macos-x86_64.dmg` | The macOS app |
| `Consort-desktop-windows-x86_64.zip`, `Consort-desktop-linux-x86_64.tar.gz` | The desktop app for Windows and Linux |
| `release.json` | The version and its protocol versions. `consort release-info` writes it |
| `SHA256SUMS` | The SHA-256 of every file above |

A tag with a `-` in it, such as `v0.3.0-rc.1`, is a pre-release. The app and the scripts use the latest release that is not a pre-release.

A manual run of the workflow (Actions > Release > Run workflow) builds the files and keeps them as a workflow artifact, with no release.

### The programs of dictation

Dictation needs `sayso-engine` and `sayso-cleanup` from Sayso. They have the GPL, so they ship as separate programs ([dictation-licenses.md](dictation-licenses.md)).

- **macOS.** `scripts/bundle-macos.sh` adds them when `CONSORT_SPEECH_ENGINE`, `CONSORT_CLEANUP_RUNTIME` and `CONSORT_SAYSO_LICENSE` name them. It signs them before the app.
- **Windows and Linux.** Put `sayso-engine` and `sayso-cleanup` in the `Consort` folder of the release file, beside the app, with the GPL text.
- **Not done yet.** `release.yml` does not build them, and Consort uses a path dependency on Sayso until Sayso is committed and published. So the workflow cannot build the desktop app today. Do step 5 of "Work in Sayso" in [DICTATION-PLAN.md](DICTATION-PLAN.md) first.

### Signing and notarization of the macOS app

The macOS jobs sign and notarize the app only when these repository secrets exist. Without them the app gets an ad-hoc signature.

| Secret | What |
|---|---|
| `APPLE_CERTIFICATE` | The Developer ID Application certificate, as a `.p12` file, encoded with `base64` |
| `APPLE_CERTIFICATE_PASSWORD` | The password of the `.p12` file |
| `APPLE_SIGNING_IDENTITY` | The name of the certificate, `Developer ID Application: Name (TEAMID)` |
| `APPLE_ID` | The Apple ID for notarization |
| `APPLE_TEAM_ID` | The team ID |
| `APPLE_APP_PASSWORD` | An app-specific password for that Apple ID |

The same variables (without the first two) work with `scripts/bundle-macos.sh` on your own Mac. Windows and Linux files are not signed.

### Build the app on your Mac

```
scripts/bundle-macos.sh              # dist/Consort.app and dist/Consort-macos-<arch>.zip
scripts/bundle-macos.sh --dmg        # also a dmg
```

The script builds `consort-desktop` in release mode, makes the bundle (`Info.plist` with the bundle ID `dev.consort.desktop`, the version from `Cargo.toml` and the minimum macOS 12), adds the icon, signs it and zips it. Start it with `open -n dist/Consort.app --args --mock` to see the app with sample data.

### The app icon

`design/logo/app-icon-dark.svg` is the Three voices mark on a rounded square in the dark canvas color (`design/mark/generate.mjs` writes it). `scripts/make-icons.sh` makes `design/logo/icons/Consort.icns` and the PNG files from it, with `rsvg-convert`, `sips` and `iconutil`. The results are in the repo, so a build needs none of these tools. The Windows build has no icon in its `.exe` yet.
