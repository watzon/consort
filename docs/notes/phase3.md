# Phase 3: hub, clients and notifications settings

These notes are for the merge into `docs/DECISIONS.md`. The plan is [FINISH-PLAN.md](../FINISH-PLAN.md), phase 3. The designs are in [design-phases-3-5.md](design-phases-3-5.md): `7JP-0` (Settings > Hub), `86L-0` (Settings > Clients), `8L0-0` (Pair a new app) and `8XN-0` (Settings > Notifications). Written on 2026-10-30.

## What was built

### Proto (`crates/consort-proto`), additive only

- New file `client_api/hub_admin.rs`: `hub.status` (`HubStatus` → `HubStatusResult { status: HubInfo }`), `hub.backup` (`HubBackup` → `HubBackupResult { progress }`) and `hub.backup_settings` (`HubBackupSettings` with optional `schedule`, `folder`, `keep` → `HubBackupSettingsResult { settings }`). Types: `HubInfo`, `HubStorage`, `BackupInfo`, `BackupSettings`, `BackupSchedule` (`frequency` off, daily or weekly; `time` as `HH:MM`; `weekday`; IANA `time_zone`), `BackupRecord`, `BackupProgress`, `BackupTrigger`, `Weekday`, and `parse_time`. The three requests are in `ClientRequest`.
- `Client` has `os` and `paired_by` (`PairedBy`: `hub_machine` or `client` with `client_id`), both optional with serde defaults. `PairClientRequest` has an optional `os`. `ClientListResult` has `online` (serde default) and its doc now says what the hub always did: it lists revoked clients too.

### Hub (`crates/consort-hub`)

- **`backup/` module.** `mod.rs` (the `Backups` state in `HubInner`, the three handlers, the start of a backup in a blocking task with a progress counter, the scheduler), `settings.rs` (`backup.json` and the schedule rule), `archive.rs` (the archive and the prune), `storage.rs` (the sizes and the free space), `restore.rs` (`consort hub restore`).
- **Backup.** SQLite's online backup API (`rusqlite` feature `backup`) copies `hub.db` in one step from a new connection, so the copy is one consistent state while the hub writes. The archive is one uncompressed tar: the manifest first, `hub.db`, `hub-secret.key`, `composio.key` and `backup.json` when they exist, then every blob. It is written as a hidden `.partial` file and renamed when complete. After a backup that worked, the hub deletes the oldest archives so that `keep` stay. Only files with the archive name count.
- **Schedule.** The hub looks every 30 seconds. A slot runs when it is after the slot of the last scheduled backup (`last_slot`), after the last schedule change, and less than an hour late, the same rule as routines. A manual backup does not move the schedule.
- **Status.** `database_bytes` is `hub.db` with `-wal` and `-shm`. `skills_bytes` is the blobs that a stored skill version names. `blobs_bytes` is the other blobs. `free_bytes` comes from `statvfs` through `rustix` on Unix.
- **Lock.** `serve` takes an exclusive lock on `<data>/hub.lock` (`File::try_lock`, std since 1.89) and holds it in `HubHandle` until `shutdown`. A second hub on the same folder now fails to start with "another hub uses the data folder". `consort hub restore` takes the same lock.
- **Clients.** A new client stores `os` and `paired_by` in its record. `client.list` fills `paired_by` from the `paired_by` column for clients that paired before, and returns `online` from the client registry. `last_seen_at` also changes when a connection ends (not during shutdown), so "Seen 2 hours ago" is the time the app left.
- **Restore.** `admin::restore(archive, data_dir, force)` reads the manifest, takes the lock, unpacks into a staging folder in the data folder, checks every blob hash and runs `PRAGMA integrity_check`, and refuses a schema newer than this hub. A folder with content needs `force`: its database, blobs, keys and `backup.json` move to `before-restore-<time>/` first. Other entries, such as `backups/`, stay.
- **Tests.** `tests/backup.rs` (7): status, the client list with `paired_by`, `os` and `online`, Back up now with the archive entries, refused settings, the schedule across three days with the prune to 2 and the late slot, the restore round trip (the old client connects to the restored hub and finds its message and its blob), and the three refusals of the restore. Unit tests in `backup/` (12): the slots, the late rule, the settings file, the sizes, the archive names and the prune.

### CLI (`crates/consort-cli`)

- `consort hub restore <backup> [--data DIR] [--force]`. It prints when the backup was made, by which hub, how many blobs, where the old files went, and how to start the hub. Errors say what to do next. Tests: the parse, and the message for a file that is not a backup.

### Client (`crates/consort-client`)

- `admin.rs`: the views (`AdminView` in `ViewState.admin`: `HubStatusView`, `ClientsView`, `ClientPairingView`, and the `AttentionView` of the notifications), the labels ("4.2 GB used, 41 GB free on the hub disk", "Finished today at 3:00 AM in 2 minutes", "Desktop app on Windows. Paired from MacBook Pro on October 20.", "Seen 2 hours ago", "At 3:00 AM", "The last 7 backups"), the countdown, the QR text, and `AdminCache` in `Cache.admin`. The cache keeps `client` feed entries now (they were dropped before), and a snapshot reload keeps them, because no snapshot holds clients.
- `hub/admin.rs`: the commands `LoadHubStatus`, `BackUpNow`, `UpdateBackupSettings(BackupChanges)`, `LoadClients`, `RevokeClient`, `ClientPairingCode` and `CancelClientPairing`. While a backup runs, one loop asks `hub.status` every second until it ends. A schedule change sends this computer's IANA time zone (`iana-time-zone`, already in the tree through `chrono`).
- `notify.rs`: the notification rules as pure logic. `attention(cache)` builds the pending approvals and decisions, every task with its status, and the newest 50 assistant timeline messages. `Notifier::observe(attention, focus, settings)` returns each event once: a new waiting item, a task that moves into Ready for review or Failed, and a message newer than the newest one it knew (a routine message counts as a routine result). Nothing that was there at the first load notifies, and nothing notifies while its place is on screen in the active window. `badge_label`, `test_notification`, `example_notification` and `example_caption` serve the settings page.
- `NotificationSettings` in `notifications.json` in the client folder (`Store::load_notification_settings`, `save_notification_settings`, and the `Client` methods of the same names).
- `mock_admin.rs`: the design content and the mock commands (a backup that finishes after 2 seconds).
- Tests: 9 in `admin.rs`, 9 in `notify.rs`, 2 in `mock_admin.rs`, 1 in `store.rs`, and 2 fake hub tests (`the_hub_page_loads_backs_up_and_changes_the_schedule`, `the_clients_page_lists_revokes_and_sees_a_new_app`).

### Desktop (`crates/consort-desktop`)

- New files: `admin.rs` (the state and the actions), `notify_platform.rs` (posting, the Dock badge, the permission), `ui/admin_parts.rs`, `ui/settings_hub.rs`, `ui/settings_clients.rs` (with the Pair a new app dialog and the QR drawing), `ui/settings_notifications.rs`, `admin_tests.rs`.
- Shared files got short additions only: `Dialog::PairClient` and the `admin` field in `app.rs` with one call each in `new`, `apply_state`, `open_settings`, `open_dialog`, `close_dialog`, `render` and the `open_screen` fallback; three routing arms and a one-line hook in `pick_select` in `ui/settings.rs`; the dialog arm in `ui/dialogs.rs`; the module lines in `main.rs` and `ui/mod.rs`; `set_app_identity` in `main.rs`.
- **Hub page:** the status line, Address with copy, Version, Data folder, the storage bar and legend, Back up now (dimmed while a backup runs), Last backup (none, running with the share, done with the file, failed with the reason), Schedule (Off, Daily, Weekly, then a day select for Weekly and a time select), Folder with Change folder (an inline field; the hub's refusal shows under the group), Keep, and the restore command with Copy command.
- **Clients page:** the rows with the This app tag, the status column and Revoke; the inline confirm in the row; the footer line. **Pair a new app:** the code, the QR code (`qrcode` 0.14.1, no default features), the countdown bar that redraws every second, Works once, the hub address with copy, Waiting for the new app, New code and Cancel. When the feed brings a client that this app paired after the code was made, the footer changes to "ThinkPad is paired. It shows in the list." with Done. An expired code says "The code expired. Choose New code."
- **Notifications page:** the five switches, Show message text (off by default), the Dock count with its preview, the permission line, the example banner with its caption (they follow Show message text), and Test notification.
- **Delivery:** `follow_admin` runs the rules at each new view state, posts through GPUI and keeps the click target of each tag. `on_system_notification_response` opens the task thread or the channel and brings the app to the front.
- **Screens:** `--mock --screen hub`, `clients`, `clients-revoke`, `pair-client`, `notifications`.
- Tests: 6 headless window tests in `admin_tests.rs` (Back up now and the schedule selects, the inline revoke and no Revoke for this app, the pairing dialog, the switches and the badge, a notification that shows once and whose click opens the thread, Test notification), 2 in `admin.rs`, 1 in `notify_platform.rs`, 1 QR test.

### Theme

- `component.admin` in `design/structure.json`, with `Admin` in `structure.rs` and `tokens.rs`, and the test `admin_tokens_keep_the_values_of_the_phase_3_designs`.

### Screenshots (`docs/screenshots/`)

`phase3-hub-dark.png`, `phase3-clients-dark.png`, `phase3-clients-revoke-dark.png`, `phase3-pair-app-dark.png`, `phase3-notifications-dark.png`.

## How the desktop posts notifications without `unsafe`

I read the pinned GPUI (`gpui-pre` 0.3.10) first. It has what we need:

- `App::show_system_notification(SystemNotification { tag, title, body, actions })`, `dismiss_system_notification(tag)` and `on_system_notification_response(callback)` with the tag of the clicked notification. The platform crates implement it: `gpui-pre-macos` with `UNUserNotificationCenter` (`objc2-user-notifications`), `gpui-pre-windows` with toasts, and `gpui-pre-linux` with the desktop notification service. The test platform records them (`shown_system_notifications`, `simulate_system_notification_response`), so the desktop tests check delivery and the click.
- On macOS GPUI posts only when the process has a bundle identifier, because `UNUserNotificationCenter` stops a process that is not in an app bundle. A `cargo run` build therefore shows no notifications. GPUI asks for permission at the first post.
- GPUI has no Dock badge. `objc2-app-kit` 0.3.2 (the version GPUI uses) has safe bindings: `NSApplication::sharedApplication(mtm).dockTile().setBadgeLabel(..)`, with a `MainThreadMarker`. Consort calls only these safe functions.
- GPUI cannot read the permission state, and the raw `getNotificationSettingsWithCompletionHandler` gives a `NonNull` that needs `unsafe` to read. `mac-usernotifications` 0.3.1 (MIT or Apache-2.0, by the author of `notify-rust`) wraps it in a safe `get_notification_settings()` and has `check_bundle()`. Consort uses only these two functions; it does not let that crate set a delegate, so GPUI's delegate keeps the clicks.
- I rejected `user-notify` (LGPL-3.0), and `notify-rust` and `mac-notification-sys` as a second posting path: GPUI already posts and routes clicks, and a second delegate would take GPUI's clicks.

Windows: GPUI posts toasts with the app identity that `main.rs` now sets (`dev.consort.desktop`). There is no Dock badge, so the count shows in the window title ("Consort (3)"), and the permission line says that Windows shows the notifications and where to change them. Linux: the same, through the desktop notification service. Neither was run on those systems in this build.

## Decisions

| # | Area | Decision | Why | Easy to change? |
|---|---|---|---|---|
| P3-1 | Hub | The backup settings and the last result live in `backup.json` in the data folder, written through a temp file and a rename (0600). No schema migration. | Phase 2 owns migration 8. The settings are a few fields that belong to the machine, and a JSON file survives a restore of an older database | Yes |
| P3-2 | Hub | A backup is one uncompressed tar archive per backup: manifest, `hub.db` from the online backup API, the keys, `backup.json`, and every blob. | One file is easy to copy off the machine. Most blobs are compressed already. The keys are needed to read the Composio key after a restore | Yes |
| P3-3 | Hub | `hub.backup` starts the backup and answers at once. The client asks `hub.status` every second while it runs. | A backup of gigabytes takes longer than the 30 second request limit. No new feed entity was needed | Yes |
| P3-4 | Hub | The schedule has a time zone from the client that set it. A slot runs once, and not when it is more than an hour late. A manual backup does not move the schedule. | The same rules as routines. The user means "3:00 AM where I am" | Yes |
| P3-5 | Hub | The archive name is the start time in the schedule's time zone, `consort-YYYY-MM-DD-HHMM.tar`, with `-2` and up for a second one in the same minute. Prune sorts by that time and number. | The design shows this name. Sorting by name is stable and needs no file times | Yes |
| P3-6 | Hub | The default backup folder is `<data>/backups`. | It always exists and is writable. The page says to choose a folder on another disk if the user wants that | Yes |
| P3-7 | Hub | A running hub holds a lock on `<data>/hub.lock`. A second hub on the same folder fails to start. | `consort hub restore` must know that no hub runs. Two hubs on one database would also break it | Yes |
| P3-8 | Hub | Free disk space comes from `statvfs` through `rustix` on Unix. On Windows the hub leaves it out, and the page shows only the used space. | `unsafe_code` is forbidden. `rustix` is in the tree already. A Windows hub is not a target today | Yes |
| P3-9 | CLI | `consort hub restore` restores into the data folder in place. With `--force` the old database, blobs, keys and settings move to `before-restore-<time>/` in that folder, not to the trash. It checks every blob hash and the database before it changes anything. | Nothing is lost by a restore. A damaged archive fails before the folder changes | Yes |
| P3-10 | Proto | `Client` carries `os` and `paired_by`, and `client.list` carries `online`. | The design says "Desktop app on Windows. Paired from MacBook Pro" and "Online now". The client record is JSON, so no migration | Yes |
| P3-11 | Hub | `last_seen_at` also changes when a client's connection ends. | "Seen 2 hours ago" should be when the app left, not when it connected | Yes |
| P3-12 | Client | The notification rules work on an `AttentionView` that the reducer builds from the cache, and run in the desktop at each new view state. | The rules stay pure and testable in the client, and the mock can drive them too. The desktop knows the focus | Yes |
| P3-13 | Client | Nothing that was there at the first load notifies. A message notifies when it is newer than the newest message the notifier knew. A task notifies when its status moves into Ready for review or Failed. | No burst of old events at start, and a page of older messages never notifies | Yes |
| P3-14 | Client | The on-screen rule covers every kind, not only messages: an approval in the open task thread does not notify while the window is active. | The user sees it already | Yes |
| P3-15 | Client | "Show message text" also covers approval summaries, decision questions, review summaries and failure reasons. The task title and the channel name always show. | They are text that the assistant or a harness wrote. The design example shows the task title with text off | Yes |
| P3-16 | Client | A message from a routine run (`author_routine_id`) counts as a routine result, not as a message. | The Routines switch is off by default, and a routine result should not notify through the Messages switch | Yes |
| P3-17 | Desktop | GPUI posts the notifications. The Dock badge uses `objc2-app-kit`, and the macOS permission state `mac-usernotifications`. Windows and Linux show the count in the window title. | See "How the desktop posts notifications without unsafe" | Yes |
| P3-18 | Desktop | The QR code holds `{"hub_url", "code"}`, the fields of `PairingInvite`, drawn on a canvas with dark modules on a light ground in both modes. | The proto names that type for this QR code. A light ground scans best | Yes |
| P3-19 | Desktop | When an app pairs with the code, the dialog stays open and says "<name> is paired" with Done. | The design says "Waiting for the new app" and leaves the end open. The user sees which app paired | Yes |
| P3-20 | Desktop | Weekly backups get a day select ("On Sunday") before the time select. | The design shows only the daily state. A weekly schedule needs a day | Yes |
| P3-21 | Desktop | Change folder opens an inline field with Save and Cancel, not a file dialog. | The folder is on the hub machine, which this app cannot browse | Yes |
| P3-22 | Desktop | The Keep select offers 1, 3, 5, 7, 14, 30 and 60 backups, plus the current count when it is another one. | A short list; the hub takes any count from 1 to 100 | Yes |

## Where it differs from the design, and why

1. **No About Consort row in the settings sidebar.** The artboards show it in the footer. The About dialog is phase 4.
2. **Protocol 1, not 3.** The design text says protocol 3. The hub and the app speak protocol 1 today, and the page shows the real number.
3. **The switch is the shared settings switch** (32 by 18 px), not the 36 by 20 px of the artboard. Every settings page uses the same switch.
4. **The setup code has no letter spacing.** GPUI text styles in the theme have no tracking. The size is the design's 34/42.
5. **The confirm row uses `tint.failed`** (16 percent), the nearest token to the design's 7 percent mix.
6. **The status line under the title shows the host of the hub URL** ("The hub runs on hub.example.com"), because the hub has no machine name to show ("the hub VPS" in the design).
7. **The design copy "or scan it with a phone" stays.** The approved decision says the Clients list does not show the mobile app before phase 6. No phone app can scan the code yet, but the approved artboard has the line, and the QR code is ready for phase 6.

## Checked by hand

- The mock app for each new screen, compared with the artboards (the screenshots above).
- Not checked: a real notification on screen. The dev build is not an app bundle, so macOS shows none, and showing one would need the permission prompt on the user's screen. The headless tests check what the app posts and the click. The Dock badge was not checked on screen either (the mock does not touch the real Dock).

## Rows for DECISIONS.md

| Area | Decision | Reason |
|---|---|---|
| Hub | Backup settings and the last result live in `backup.json` in the data folder, not in `hub.db` | No schema migration in phase 3; the settings belong to the machine |
| Hub | A backup is one tar archive: manifest, `hub.db` from SQLite's online backup, the keys, `backup.json` and every blob | One file to copy; a consistent database while the hub runs; the keys read the Composio key after a restore |
| Hub | `hub.backup` answers at once; clients poll `hub.status` every second while it runs | A large backup takes longer than a request; no new feed entity |
| Hub | Scheduled backups use the client's time zone, run once per slot, and skip a slot more than an hour late | The same rules as routines |
| Hub | Archive names are `consort-YYYY-MM-DD-HHMM[-N].tar` in the schedule's time zone; prune keeps the newest N by name | The design's name; stable sort |
| Hub | A running hub holds `<data>/hub.lock`; a second hub and a restore refuse that folder | Restore must know no hub runs; two hubs would break the database |
| Hub | Free disk space from `statvfs` (rustix) on Unix only | No `unsafe` in Consort |
| Hub | `last_seen_at` also changes when a client disconnects | "Seen 2 hours ago" means when the app left |
| CLI | `consort hub restore` checks hashes and the database first, and with `--force` moves the old files to `before-restore-<time>/` | Nothing lost; a damaged archive fails before changes |
| Proto | `Client.os`, `Client.paired_by` and `client.list` `online` | The Clients page needs them; JSON record, no migration |
| Client | Notification rules are pure logic in `consort-client` (`notify`) on an `AttentionView`; the desktop delivers | Tested once, shared with mobile later |
| Client | Nothing notifies for what was there at the first load, or while its place is on screen in the active window | No burst at start; no notification for what the user sees |
| Client | Message text off hides every written text (messages, summaries, questions, reasons); task titles and channel names show | The design example; privacy |
| Client | Routine messages count as routine results, not messages | The Routines switch is off by default |
| Client | Notification switches live in `notifications.json` in the client folder | The approved design: settings are per client |
| Desktop | GPUI posts notifications and routes clicks; `objc2-app-kit` sets the Dock badge; `mac-usernotifications` reads the permission; Windows and Linux show the count in the window title | No `unsafe` in Consort; GPUI keeps the click delegate |
| Desktop | The QR code holds `PairingInvite` JSON (`hub_url`, `code`) | The proto type for this code |
| Desktop | The pairing dialog says which app paired, with Done | The design leaves the end state open |
| Desktop | Weekly backups have a day select; Change folder is an inline path field | A week needs a day; the folder is on the hub machine |

## What is left

- **A live notification check from an app bundle.** Phase 5 makes the `.app`. Then check a banner, the permission prompt, the click, and the Dock count by hand.
- **Windows and Linux notifications** were built through GPUI and not run on those systems.
- **Free disk space on a Windows hub** is not shown.
- **A backup cannot be stopped** once it runs. It ends or fails on its own.
- **The hub status has no feed entity.** Another client sees a backup that this client started only when it opens the page or polls.
- **Backups are not encrypted.** The archive holds the keys and the database, so the backup folder needs the same care as the data folder (it is made with mode 0700 when the hub makes it).
