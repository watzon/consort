# Windows checks

These notes record the Windows checks of Consort on the test machine `bigboi` (Windows 11, x64, Windows PowerShell 5.1, Rust 1.96.1, no `cargo-nextest`). The code was `main` (commit `ca536cb`) plus the changes in this branch. The change files went to bigboi with `scp` on top of the committed history, because the agent does not commit.

## Result

| Check | Result |
|---|---|
| `cargo build --workspace` | Passed. One warning (dead code in `Permission`). Fixed. |
| `cargo test --workspace` | First run: 9 tests failed in 8 targets, and `consort-hub` did not compile its tests. All fixed. Last full run: 1565 passed, 0 failed, 25 ignored. |
| `cargo clippy --workspace --all-targets -- -D warnings` on Windows | First run failed with unused imports and dead code in test files. Fixed. Passes now. |
| `consort worker install/status/uninstall --dry-run` | Worked. The task XML had a bug with the user name. Fixed. |
| `scripts/install/worker.ps1` | Passed (parse, dry run, scriptblock form, fake release, bad checksum). No change needed. |
| Desktop app with `--mock --screen add-worker` and `--screen notifications` | Both screens draw. `scripts/windows-shot.ps1` had a bug. Fixed. |
| `scripts/check.sh` on the Mac | Passed. |

## What failed and how it was fixed

### Build

`Permission` in `crates/consort-desktop/src/notify_platform.rs` has five variants that only macOS builds. On Windows and Linux they are dead code, and `-D warnings` fails. The enum now has `#[cfg_attr(not(target_os = "macos"), allow(dead_code))]`.

### Task Scheduler XML: wrong user name (real bug)

Over SSH, Windows sets `USERDOMAIN` to `WORKGROUP` for a local account. The service code wrote `WORKGROUP\cawat` into `<UserId>`. That name does not resolve to a SID (`NTAccount.Translate` fails on bigboi), so `schtasks /Create /XML` would reject the task. The name `BIGBOI\cawat` resolves.

Fix: `windows_account()` in `crates/consort-cli/src/service.rs`. When the domain is `WORKGROUP` (or missing), it uses `COMPUTERNAME`. A real domain stays. Two unit tests cover it.

After the fix, the dry run shows `BIGBOI\cawat`. The Task Scheduler COM object (`Schedule.Service`, `NewTask().XmlText`) accepted the XML. This check parses the XML and registers nothing. It read back the user, the logon type (interactive token), the restart rule (`PT1M`, 999 times) and the action (`powershell.exe -NoProfile -NonInteractive -WindowStyle Hidden -Command "& '...consort.exe' 'worker' 'run' '--log-file' '...'; exit $LASTEXITCODE"`).

The dry runs of `status` (`schtasks /Query /TN "Consort worker" /FO LIST /V`) and `uninstall` (`schtasks /End`, `schtasks /Delete /F`, and removal of `worker-task.xml`) look right. No task was created.

### Tests that failed on Windows

All of these were test code or test helpers, except the first row, which is a small product change (`unix_script` is only used on Linux, so it did not break a real update).

| Test | Cause | Fix |
|---|---|---|
| `consort-client` `update::apply::tests::the_linux_script_waits_unpacks_replaces_and_starts_the_app` | `unix_script` joined paths with `Path::join`, which uses `\` on Windows | `unix_script` joins the paths as text with `/`, like `windows_script` does with `\` |
| `consort-client` `blob_transfer::a_wrong_token_gives_unauthorized_and_stores_nothing` | The fake hub answered 401 before it read the upload body. Windows resets such a connection, so the client saw `Unreachable` | The fake hub reads the body, then answers |
| `consort-desktop` `files_tests::the_light_box_opens_steps_and_closes` | The light box covers the app but not the Windows title bar (by design, so the window can always move and close). The test expected the full window height | The test subtracts the title bar height when the window has controls |
| `consort-harness` `acp::tests::the_skill_folder_goes_into_the_opencode_config` and `codex::tests::fake_server::the_skill_folder_is_an_extra_root_before_the_thread_starts` | The tests wrote the expected path with `/`. The code uses `join`, so a Windows path has `\` (and JSON escapes it) | The tests build the expected path with `join` and compare the JSON text |
| `consort-hub` `skills::upload::tests` (two tests) | The `zip` crate writes the DOS system on Windows, and a reader then ignores the Unix mode (executable bit, device files) | The test zip writer sets `System::Unix`. Real zips with a mode come from Unix |
| `consort-hub` `skills::fake::TestRepo` | `write_executable` existed on Unix only, so the lib tests did not compile. `url()` made `file://C:\...` | `write_executable` records the file and `commit` runs `git update-index --chmod=+x`. `url()` makes `file:///C:/...`. Two skill flow tests that were `cfg(unix)` only because of the missing helper now run on Windows too |
| `consort-worker` `accounts_tests::an_opencode_account_runs_with_its_own_home` | The test wrote `xdg/data` with `/` | The test uses `join("xdg").join("data")` |
| `consort-worker` `attachments::a_git_workspace_does_not_see_the_worker_folders` | The clone used the system git settings of Windows (`core.autocrlf=true`, CRLF files). The test checked `git status` with settings off, so git saw every file as changed | The test passes `-c core.autocrlf=true` on Windows |
| `consort-hub` `integration` `heartbeat::a_silent_worker_goes_offline_after_sixty_seconds` | Flaky under load, see "What is left" | None yet |

Clippy on Windows also failed on unused imports and unused functions in two test files that use them in `cfg(unix)` tests only (`consort-worker/src/accounts_tests.rs`). Those imports have `#[cfg(unix)]` now.

### `scripts/windows-shot.ps1`

The script had a parameter named `$Args`. `$Args` is an automatic variable in PowerShell, so the program always started with no arguments. With no `--mock` the app tried to connect and the script said "no window". The parameter is now `$AppArgs` with the alias `Args`, so old command lines still work. The result line now also has the process ID and the window title, so you can stop the app by PID (`taskkill /PID <id> /F`).

The script makes and deletes one Task Scheduler task, `ConsortShot`, because a program must start in the interactive session to open a window (over SSH, the app says "cannot open windows while headless"). The task is gone after the run.

## `scripts/install/worker.ps1`

- The parse check (`[System.Management.Automation.Language.Parser]::ParseFile`) found 0 errors.
- `-DryRun` printed the right steps. The scriptblock form from `docs/install.md` (`& ([scriptblock]::Create(...)) -Hub ... -Code ... -DryRun`) works.
- The fake release ran in a temp folder with `CONSORT_DOWNLOAD_BASE` (a `file:///` URL) and `CONSORT_INSTALL_DIR`, and `-NoService`. A zip with a fake `consort.exe` and a `SHA256SUMS` file was the release. The good checksum installed the file. A second run replaced it in place and left `consort.exe.old`. A wrong checksum stopped with exit code 1 and installed nothing. A missing `-Hub` stopped with the right message.
- The script writes the user `Path` to the registry (`SetEnvironmentVariable(..., 'User')`) and has no flag to skip that. To keep the machine clean, the fake run used a copy of the script with that block cut out. The copy and the folder are deleted. The real script was not run without `-DryRun`.

## Desktop app on Windows

`consort-desktop` built and ran. The window has the custom title bar with the minimize, maximize and close buttons. The screenshots are in `docs/screenshots/`:

- `windows-add-worker.png`: the Add worker dialog, step 1. Windows is the selected system, with the line "Runs as a Task Scheduler task."
- `windows-notifications.png`: Settings > Notifications. The row reads "Show a count in the window title" (not "Dock badge"). The permission line reads "Windows allows notifications from Consort. To change their style or sound, open Settings > System > Notifications."

**The window title count fallback.** The code is right, but the run could not show it. The app does not set the window title in `--mock` mode (`apply_window_badge` returns early when `demo` is true), so the title stays "Consort" there. The check in this run:

- `notify_platform::HAS_DOCK_BADGE` is false on Windows, so the fallback is on.
- A new unit test, `admin::tests::the_window_title_shows_the_count_where_there_is_no_dock_badge`, checks the title text ("Consort", "Consort (3)", "Consort (99+)"). `apply_window_badge` now calls the small function `window_title` that the test checks.
- The existing test `a_switch_changes_the_notification_settings` checks the count (3) and that it goes away when the setting is off.
- Not checked: a real window with a count in its title bar, and real toasts. That needs a paired hub and a running task. See "What is left".

## Last runs

- bigboi, `cargo test --workspace --no-fail-fast`: 1565 passed, 0 failed, 25 ignored. `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check`: clean.
- Mac, `scripts/check.sh` (the whole workspace): "All checks passed". The two bigboi-only helper changes (`write_executable`, the two skill flow tests) also pass there.

## What is left

- **Flaky heartbeat test on Windows.** `heartbeat::a_silent_worker_goes_offline_after_sixty_seconds` failed once in a full `cargo test --workspace` run (`no response to hello`) and passed in 6 of 6 runs alone and in the next full runs. It uses `start_paused = true` with real sockets. When the machine is busy, the paused clock jumps ahead while the real socket still works, and the 10-second wait (`WAIT`) for the `hello` response ends too early. A fix needs a change in `consort-hub/tests/common/mod.rs` (for example, wait with the real clock for the handshake). It is not a product bug.
- **Real toasts and the title count** are not checked with a real hub and a paired client on Windows. Needs a person at the machine, or a hub that bigboi can reach.
- **`worker.ps1` against a real release.** There is no release yet. The fake release covers the download, checksum and unpack steps. The step `consort worker install` (pairing and the real task) needs a real hub and a code.
- **`consort worker install` for real.** The task XML is checked by the Task Scheduler parser, but nobody ran `schtasks /Create` (the rules for this run forbid new tasks on bigboi). Do it once by hand: `consort worker install --hub <hub> --code <code>`, then `schtasks /Query /TN "Consort worker" /FO LIST /V`, then `consort worker uninstall`.
- **The task starts `powershell.exe` with `-WindowStyle Hidden`.** A console window can flash at sign-in. A launcher without a console window (a `windows_subsystem = "windows"` build, or `wscript`) would fix it. This is a polish item.
- **A hub on Windows** does not show free disk space (decision P3-8). Unchanged.
- **`cargo-nextest`** is not installed on bigboi, so the run used `cargo test`. `scripts/check.sh` falls back to it by design.

## Second run (2026-10-30)

The code was `main` at commit `2057d66` (phases 2, 4, 4b and 5 and the smaller hub and worker items) plus the changes in this branch. The history went to bigboi as a git bundle (`git bundle create ca536cb..windows-fixes-2`, `scp`, `git fetch` of the bundle in `C:\Users\cawat\Projects\consort`). The branch change files went over as one `git diff HEAD` patch and `git apply`. The ssh user is `cawat@bigboi.local` (the `bigboi` host entry in `~/.ssh/config` names another user). The default shell over ssh is `cmd.exe`.

### Passed

- `cargo build --workspace`: no warning.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace --no-fail-fast` (no nextest on bigboi): 1845 passed, 0 failed, 25 ignored, after the fixes below.
- The persistent client cache (`consort-client` `persist.rs`) passes all its tests on Windows. This includes moving a broken file aside, the newer schema file and `delete`. `CacheFile::open` drops the SQLite connection before it returns an error, so the file can be renamed. No change was needed.
- Hub backup and restore tests pass.
- Desktop app with `--mock --screen proactivity`, `hub-offline` and `channel-settings`: all three draw. The screenshots are `docs/screenshots/windows-proactivity.png`, `windows-hub-offline.png` and `windows-channel-settings.png`. The title bar has the Windows controls. The hub-offline screen shows the "Cannot reach the hub" banner with "Try again", and "Messages wait here until the hub is back" under the composer. Each app was stopped by PID.
- `scripts/check.sh` on the Mac: "All checks passed in 111s".

### Failed and fixed

| Test | Cause | Fix |
|---|---|---|
| `consort-hub` `prompts::tests::the_coordinator_prompt_has_a_working_on_your_own_section_with_the_rules` | Git on Windows has `core.autocrlf=true`, so `prompts/coordinator.md` had CRLF line ends. The code embeds it with `include_str!` and looks for `"# Working on your own\n"`. This is a product risk too: the model prompt would have `\r\n` in a Windows build. | `.gitattributes` sets `eol=lf` for `crates/consort-hub/prompts/**` and for the embedded `crates/consort-hub/src/store/*.sql`. |
| `consort-hub` `skills::remote_tests::git_gives_up_on_a_server_that_never_answers` | After the time limit, `run_git` dropped the child, which ended `git.exe` but not its helper `git-remote-http.exe`. The helper kept the temp folder as its current folder, so Windows could not remove `skill-fetch-*`. Two folders stayed. This is a real leak in a Windows hub. | `run_git` in `crates/consort-hub/src/skills/git.rs` now reads the pipes in two tasks and waits on the child itself. At the time limit (and the size limit) `stop` runs `taskkill /T /F /PID` on Windows, kills the child and waits until it has ended. |
| `consort-hub` integration `skills::a_client_searches_previews_installs_and_updates_a_skill` | "no response to skill_directory.search" after 10 s in the full run. Alone it takes 1.8 s, and 3 of 3 runs of the whole binary passed. Git starts slowly on Windows and the search fetches a repo. | `WAIT` in `crates/consort-hub/tests/common/mod.rs` is 40 s on Windows and 10 s elsewhere. |

### Left

- `heartbeat::a_silent_worker_goes_offline_after_sixty_seconds` did not fail in this run, but the cause from the first run (paused clock with real sockets) is still there.
- The ssh command shell is `cmd.exe`. A task that needs `powershell` must call it.
- The checks that need a person at the machine are unchanged: real toasts, the window title count with a paired hub, `consort worker install` for real, and `worker.ps1` against a real release.
- `consort-desktop` `--mock` shows saved data, but the screens do not show the SQLite file on disk. Nothing in the run opened a real cache file in the app. The tests cover it.
