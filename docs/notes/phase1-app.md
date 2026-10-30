# Phase 1, app part: files and attachments

These notes are for the merge into `docs/DECISIONS.md`. The plan is [FINISH-PLAN.md](../FINISH-PLAN.md): the two "First" sections and phase 1. The backend part is in [phase1-backend.md](phase1-backend.md). Written on 2026-10-29. Only `consort-client`, `consort-desktop` and `consort-theme` changed, with `design/structure.json` and the docs.

## What was built

### The top tint and the larger sidebar (`consort-theme`, `design/structure.json`)

- **Top tint.** `layout.top_tint` has four stops: the canvas at 94 percent at 0 px, 84 percent at 56 px, 50 percent at 108 px, and clear at 184 px. The view draws three gradient layers, one for each pair of stops (`ui::timeline::top_tint`). The solid zone is gone (`fade_top_solid` and `fade_top_blend` are removed). `layout.content_top` (92 px) is the space above the first item at rest, below the mark and the pill. The overlay with the mark, the pill and the top actions keeps its place on the tint.
- **Sidebar.** New block `component.sidebar`: inset 10, rows 7/10 with a 10 px gap (36 px), 15/22 row text, 18 px icons, `lg` corners on every row, 13/18 group labels (weight 500), 20 px above a group, a 52 px title bar, 14/20 worker rows (30 px), a 28 px avatar. `layout.sidebar_width` is 288. The type styles follow the text size setting. The settings rail uses the same rows and labels.
- **File tokens.** New block `component.files` for the chips, the bubbles, the drop state, the light box and the rows of the Files tab. New opacity tokens: `failed_border`, `progress_track`, `drop_fill`, `nav_disabled`, `you_tile`. New derived color `lightbox` (black at `derive.lightbox_alpha`, 0.92).
- Tests: `the_top_tint_gets_lighter_from_the_top_to_clear`, `file_tokens_keep_the_values_of_the_phase_1_designs`, `the_sidebar_text_follows_the_text_size`, and the updated component and layout tests.

### Client (`consort-client`)

- **Blob transfer** (`blob.rs`, built by a subagent from my brief). `BlobTransfer` hashes a file as a stream, asks the hub with `HEAD /v1/blobs/{sha256}`, uploads with `PUT` and a `Content-Length` in 256 KiB pieces with progress, and downloads with `GET` into a cache by hash (temp file, hash check, rename). It refuses a file over 256 MiB before any request. Each error says what happened and what to do next.
- **HTTP token** (`hub/files.rs`). The client gets a token with `auth.http_token` and keeps it until one minute before it ends. When the hub answers 401, the client gets a new token and tries once more.
- **Uploads** (`uploads.rs`). A pure state machine: Preparing, Uploading with its share, Ready with the blob, Failed with the message. Each target is a composer, a reply box or a folder of channel files. Each start has an attempt number, so a result from a removed or restarted upload does nothing. `Uploads::gate` tells a send to go, to wait, or to stop.
- **Commands.** `Attach`, `RetryUpload`, `RemoveUpload`, `LoadChannelFiles`, `MoveChannelFile`, `DeleteChannelFile`, `SaveToChannelFiles`, `FetchBlob` (with a named copy for Open) and `SaveBlob`. `SendMessage` and `ReplyToTask` take upload IDs. The client waits until the uploads are on the hub, then puts the blobs in the outbox entry (`PendingSend.attachments`, serde default for old outbox files), and `message.send` carries them. A failed upload stops the send with "The upload of X failed. Choose Try again on the file, or remove it, then send again".
- **Paste.** `AttachInput::Pasted` writes the picture to `<client folder>/blobs/pasted/`, then uploads it like a file.
- **Channel files in the cache** (`cache.rs`). The cache keeps `channel_file` entries of the channels whose files it holds (from a channel snapshot or `files.list`), reduces upserts, `deleted_at` and deletes, keeps the newer feed change over an older answer, and drops the files with their channel. Before, `cache.rs` dropped them.
- **Views.** `MessageView.attachments` and `PostBody::Attachments` with `AttachmentView` (type and size, a local path when the blob is in the cache, and the task of an artifact). `ViewState.uploads`, `channel_files` and `files_loaded`. `files::listing` gives folders, files and the artifacts grouped by task. `chat::rows` gives `ChatRow::Attachment` rows: your pictures and files before your text, the assistant's after its text.
- **Skill zip upload.** `skill.preview` and `skill.install` with `SkillInstallInput::Upload` check the name (`.zip`) and the 25 MB limit, upload the zip through the blob upload, and send `Upload { blob_sha256, size, file_name }`. The refusal in `extensions_view.rs` is gone.
- **Mock.** `mock_files.rs`: the files of house-lights, the photos (small SVG drawings in the temp folder), the artifacts of the roofline task, the composer uploads and the reply upload, and the file commands with simulated progress.
- **Fix found by the tests.** The outbox sent `attachments: Vec::new()` to the hub. It sends the stored blobs now.
- Tests: `uploads::tests` (6), `files::tests` (3), `mock_files::tests` (1), two cache tests for channel files and artifact attachments, a chat rows test, `format` file sizes, and four fake hub tests: `an_attached_file_goes_up_once_and_the_message_carries_it`, `a_failed_upload_stops_the_send_until_it_is_removed`, `channel_files_load_upload_move_download_and_delete`, `a_skill_zip_goes_up_before_the_preview`. The fake hub now has `auth.http_token`, `/v1/blobs/{sha256}`, `files.*`, and uploads for skills. The blob module has 12 unit tests and 13 tests in `tests/blob_transfer.rs`.

### Desktop (`consort-desktop`)

- **Composer** (`ui/attachments.rs`, `ui/timeline.rs`). The add button opens the add menu (Upload from this computer, Choose from channel files). Paste takes pictures and copied files and leaves text to the box (a capture of the `Paste` action). Chips sit in the field above the text: picture thumbnails with a remove badge on hover, file chips with type and size, an upload with its bar and share, a failed upload with its message and Try again. Send waits for the uploads.
- **Drop.** A drop anywhere over the chat column attaches to the composer. While files are over the column, the composer grows and shows the dashed accent state with "Drop to attach 3 files" and "Up to 256 MB each". A drop on the Channel panel uploads to its open folder. A drop on a task thread attaches to the reply box.
- **Bubbles.** Pictures as 228 by 168 tiles, two in a row. File bubbles (280 px in your lane, 360 px in the assistant lane). An artifact names its task with the status glyph and its size. Save and Open show on hover. The menu is Open, Save, Save to channel files.
- **Light box.** Name and details, Save, Open, close, Previous and Next, "1 of 2", the arrow keys and Escape.
- **Channel panel** (`ui/channel_panel.rs`). The `#` top action opens it; it shares the right side with the Tasks panel. Header with name and purpose. Tabs Files, Routines and Memory, where Routines and Memory are disabled with the tooltip "Comes with phase 2". Files: the summary, Upload, folders (they open in place, with a back button), files with size and date, uploads with their bar, artifacts grouped by task (a click opens the task thread), the row menu Open, Download, Rename, Move, Delete (Delete asks first), the drop state, the empty state that is a drop zone, and the hint at the bottom. The list scrolls with fades.
- **Dialogs.** Delete file, Rename file, Move file (a list of folders), Choose from channel files.
- **Task thread.** The reply box has the chips, the add button (its menu flips upward), paste and drop. Thread posts show their attachments as compact rows.
- **Settings > Skills > Add skill > Upload** works through the client upload.
- **Screens** (`--mock --screen <name>`): `top-tint` is `tutti`, `attachments`, `drop`, `bubbles`, `lightbox`, `files`, `files-menu`, `files-drop`, `files-empty`, `thread-attach`, `add-skill-upload`.
- **AGENTS.md** rule 2 lists the new menus. Rule 3 describes the tint.
- Tests (`files_tests.rs`, 10): the sidebar and settings rail sizes, the drop over the chat, the chips and a failed upload that stops the send, a send that waits for its uploads, bubbles with pictures, files and artifacts (and the artifact menu), the light box, the Channel panel with its menu and Delete, Rename and Move, the reply box with its file, and the skill zip check. Also `the_top_tint_lies_under_the_mark_and_the_first_bubble_starts_below_the_pill` (replaces the solid-zone test), `the_file_menus_keep_the_names_and_the_order`, and paste and drop label tests.

### Screenshots (`docs/screenshots/`)

`phase1-top-tint-sidebar-dark.png`, `phase1-composer-attachments-dark.png`, `phase1-drop-on-composer-dark.png`, `phase1-bubbles-attachments-dark.png`, `phase1-lightbox-dark.png`, `phase1-channel-files-dark.png`, `phase1-channel-files-menu-dark.png`, `phase1-channel-files-drop-dark.png`, `phase1-channel-files-empty-dark.png`, `phase1-thread-reply-attachment-dark.png`, `phase1-add-skill-upload-dark.png`, `phase1-settings-rail-dark.png`.

## Where it differs from the design, and why

1. **The mark is 56 px, not 44 px.** The phase 1 artboards draw a 44 px mark at 6 px. The app keeps `chat.mark` (56) and `chat.mark_top` (4), because the approved chat design and its tests use them, and the task did not ask to change the mark.
2. **The sidebar keeps the "New channel" row.** The base artboard has it too. The decision to remove it belongs to phase 4.
3. **A picture in the reply box is a thumbnail chip.** 9QD draws a wider chip with the thumbnail, the name and the size. The composer and the reply box share one chip, so they look the same.
4. **The add menu has no hint line.** 9QD has "You can also drop or paste files in the reply box." under the two items. The shared menu layer has no place for a note line. The empty Files tab and the panel hint say it instead.
5. **No New folder button in the Files tab.** 8PM has one. A folder exists only as a part of a file path (`files.put` takes a path), so an empty folder cannot exist. An upload into an open folder, Rename and Move make folders.
6. **The light box details have no pixel size.** 8EA says "JPEG, 3.1 MB, 4032 by 3024. You sent it today at 7:40 PM." The app does not read the picture size before it draws. The details are "JPEG, 3.1 MB. You sent it at 7:40 PM."
7. **The Previous and Next buttons are on `surface.raised`.** 8EA uses white at 6 percent, which is not a token.
8. **The tabs show the file count only.** Routines and Memory have no data in phase 1, so they show no count.
9. **An upload in the Files tab is listed after the files**, with "Uploading, 62%" and a cancel button, not in name order.
10. **HEIC and TIFF show as file chips.** GPUI cannot draw them.
11. **Drawn without a design:** the Choose from channel files dialog (a list of every file of the channel; a click attaches it), the Move dialog (a list of the folders with the chosen one checked), the delete confirmation (the design notes say it is not drawn), and the drop state of a full Files tab (an overlay on the list with the count and the folder).

## Decisions

| # | Area | Decision | Why | Easy to change? |
|---|---|---|---|---|
| P1A-1 | Theme | The top tint is four stops in `layout.top_tint` (94, 84, 50 and 0 percent at 0, 56, 108 and 184 px), drawn as three gradient layers. `layout.content_top` (92) replaces `fade_top_solid` and `fade_top_blend` as the space above the first item at rest. | The design asks for a progressive tint with stops as tokens, and GPUI gradients have two stops. The first item must still start below the pill | Yes |
| P1A-2 | Theme | The sidebar sizes are a `component.sidebar` block with its own text styles (15/22, 13/18 weight 500, 14/20), which follow the text size setting. The settings rail uses the same tokens. | The new sizes are not in the type scale, and they must scale like the rest of the text | Yes |
| P1A-3 | Client | The client checks for a blob with `HEAD /v1/blobs/{sha256}`. 200 skips the upload. 404, 405 and 501 upload it. | The hub's axum route answers HEAD on its GET route, so no hub change is needed. A hub that cannot answer still works | Yes |
| P1A-4 | Client | The HTTP token is kept until one minute before it ends. A 401 gets a new token and one more try. | A long upload must not start with a token that ends on the way | Yes |
| P1A-5 | Client | The local blob cache is `<client folder>/blobs/<sha256>`. Open uses a named copy in `blobs/open/<sha256>/<name>`, and pasted pictures go to `blobs/pasted/`. | Another app needs a file name with an extension to open a file. The client folder already belongs to the app | Yes |
| P1A-6 | Client | The uploads live in the hub backend of the client, not in the cache or the outbox. A message in the outbox keeps the blobs of its uploads (`PendingSend.attachments`). | An upload that runs cannot go on after a restart, but a message that waits for the hub must keep its files | Yes |
| P1A-7 | Client | A send waits in the client until its uploads are on the hub. A failed upload stops the send with an error, and the text stays in the box. | The design says that Send waits. A message must not go out without a file that the user attached | Yes |
| P1A-8 | Desktop | Routines and Memory show as disabled tabs with the tooltip "Comes with phase 2", not hidden. | The panel keeps its final shape, so phase 2 adds content and no layout. A disabled tab says why it does nothing | Yes |
| P1A-9 | Desktop | Drop targets: the chat column attaches to the composer, the Channel panel uploads to its open folder, a task thread attaches to the reply box. Only the target under the pointer shows a drop state. | The approved decision says a drop anywhere over the chat attaches. The panels are their own targets | Yes |
| P1A-10 | Desktop | Paste is a capture of the `Paste` action on the message box. Pictures and copied files attach. Text pastes as usual. | The GPUI Kit text area has no paste hook, and the action reaches the box before the text area | Yes |
| P1A-11 | Desktop | Save to channel files puts the file in the top folder with its own name. A file with the same path is replaced, as `files.put` does. | It is one click in the menu. Move and Rename can put it elsewhere | Yes |
| P1A-12 | Desktop | Save opens the system save dialog in `~/Downloads` (or the temp folder) with the file name. | The usual place for a download | Yes |
| P1A-13 | Client | File sizes show kilobytes with no decimal and megabytes with one ("48 KB", "2.4 MB"), with `format::file_size_label`. The older `size_label` stays for the connector activity. | The designs write sizes this way | Yes |
| P1A-14 | Desktop | File dates are "Today", else the short date ("Oct 24"), with the year when it is not this year. | The design notes | Yes |
| P1A-15 | Client | A skill zip is checked for the `.zip` name and the 25 MB limit before the upload. | The hub would refuse it after the upload | Yes |
| P1A-16 | Desktop | A picture chip shows a thin progress bar at its bottom while it uploads. | 7D6 draws only file chips while they upload, but a picture uploads too | Yes |
| P1A-17 | Theme | The light box is black at 0.92 (`derive.lightbox_alpha`), in both modes. | 8EA is near black, and a picture reads best on black | Yes |

## Proto and backend notes

- No change to `consort-proto` was needed. `message.send` already has `attachments`, and `SkillInstallSource::Upload` already exists.
- `docs/protocol.md` does not say that `HEAD /v1/blobs/{sha256}` works. The hub answers it today because axum routes HEAD to the GET handler (which reads the whole blob). Please add one line to the Blobs section, and consider a HEAD handler that only checks that the file exists.

## Checks

- `cargo fmt --all --check`: clean.
- `cargo clippy -p consort-client -p consort-desktop -p consort-theme --all-targets -- -D warnings`: clean.
- `cargo test -p consort-client -p consort-desktop -p consort-theme`: see the final report for the counts.
- The mock app was run for each new screen and compared with the artboards (the screenshots above).

## What is left

- Download progress does not show. Open and Save wait without a bar.
- An upload cannot be cancelled on the wire. Remove takes it away, and its result does nothing, but the transfer finishes in the background.
- Uploads do not survive a restart of the app.
- A message that the hub refuses after it left the composer keeps its text in the failed sends, but not its attachments.
- Choose from channel files attaches one file per click.
- A real drag from Finder was not tried by hand. The tests send the platform file drop events.
- Light mode screenshots were not taken.
- The Security audits row of an uploaded skill says "From the directory that lists it.", which is wrong for a zip. That row comes from the existing skill dialog.
