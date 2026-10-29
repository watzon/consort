# Rust GUI stack for a Duet-style AI assistant app

Research date: 2026-10-28. Read-only. All numbers come from live GitHub API, crates.io API, npm registry, and project docs read during this session.

Labels: **FACT** means I read it in a primary source (URL given). **INFERENCE** means I concluded it from facts. **UNVERIFIED** means I could not confirm it in this session.

Scope change applied: the user decided GPUI is desktop-only. Mobile will use Expo or native Swift/Kotlin. Question 3 stays short. Question 7 is now about sharing a Rust client core with the mobile apps.

---

## Summary table

| Topic | Result |
|---|---|
| "GPUI Kit" | `longbridge/gpui-kit` (renamed from `longbridge/gpui-component`). Crate `gpui-kit` 0.8.1 is the umbrella. Apache-2.0. 18.7k stars. |
| GPUI on crates.io | Stale. `gpui` 0.2.2 is from 2025-10-22. Zed set `publish = false`. Apps use git deps or `gpui-pre` snapshot crates. |
| Chat parts in Kit | Message, MessageGroup, Bubble, Attachment, MessageScroller, TextView (Markdown), Textarea, Speech, Sidebar, Dock, Sheet. |
| Not in GPUI or Kit | System tray, global hotkeys, auto-update, WebSocket client. |
| Closest real app | `zeronsh/zeron` (GPUI + gpui_tokio + tokio-tungstenite, agents control plane). |
| Mobile core sharing | UniFFI 0.32.2 is the base. `uniffi-bindgen-react-native` 0.31.0-6 (prerelease) for Expo. matrix-rust-sdk is the main precedent. |

---

## 1. What is "GPUI Kit"?

### 1.1 Answer

**FACT.** "GPUI Kit" is `longbridge/gpui-kit`. It is the new name of `longbridge/gpui-component`. The GitHub API for `repos/longbridge/gpui-component` returns `full_name: longbridge/gpui-kit`. The crates.io entry for `gpui-component` has `repository = https://github.com/longbridge/gpui-kit`. Its description reads "GPUI Component: the styled component library of GPUI Kit".

- Repo: https://github.com/longbridge/gpui-kit
- Site and docs: https://gpui-kit.com
- Crates: https://crates.io/crates/gpui-kit , https://crates.io/crates/gpui-component

**INFERENCE.** This is the one the user means. It matches the literal name "GPUI Kit". It is the only large GPUI component library. The older name `gpui-component` is what most existing articles use.

### 1.2 Repo facts (GitHub API, 2026-10-28/29)

| Item | Value |
|---|---|
| Stars / forks | 18,740 / 1,161 |
| Open issues | 133 |
| Created | 2024-06-13 |
| Last push | 2026-10-29 |
| Latest release | v0.8.1, 2026-10-26 |
| Release rhythm | v0.7.0 2026-09-28, v0.7.1 10-05, v0.7.3 10-12, v0.8.0 10-19, v0.8.1 10-26 (weekly) |
| License | Apache-2.0 for code (`LICENSE-APACHE`). Docs prose is also CC BY 4.0 (`LICENSE-DOCS.md`). GitHub shows "NOASSERTION" because there are two license files. |
| Production use | README says it powers Longbridge Pro, a shipped commercial desktop app (closed source). |

### 1.3 Crate layout

**FACT** (README, https://github.com/longbridge/gpui-kit/blob/main/README.md):

- `gpui-kit`: the one crate apps depend on. Pins a GPUI snapshot and re-exports GPUI, base, component, and assets. 0.8.1, 198k downloads.
- `gpui-base`: unstyled behavior, state, and infrastructure (like Base UI).
- `gpui-component`: the styled UI system (like shadcn). 0.8.1, 371k downloads.
- `gpui-shell`: JavaScript extension runtime hosted by Rust.
- `gpui-webview`: Wry-based WebView, experimental.
- `gpui-fast` (https://github.com/longbridge/gpui-fast): optional alternative GPUI backend with rendering and layout optimizations. Enabled by a feature flag.

### 1.4 Component list

**FACT** from https://gpui-kit.com/component (read this session):

- Basic: Accordion, Alert, Attachment, Avatar, Badge, Bubble, Button, Checkbox, Collapsible, DropdownButton, Icon, Image, Kbd, Label, Marker, Message, MessageScroller, Pagination, Progress, Questionnaire, Radio, Rating, Skeleton, Slider, Spinner, Stepper, Switch, Tag, TextView, Toggle, Tooltip.
- Form: Input, Textarea, Editor, Select, Combobox, NumberInput, DatePicker, TimeField, OtpInput, Speech, ColorPicker, Form.
- Layout: DescriptionList, GroupBox, Root, Theme, Dialog, Notification (toasts), Popover, Resizable, Scrollable, Sheet, Sidebar, StatusBar, Toolbar.
- Advanced: Diff, Calendar, Carousel, Command, Chart, List, Menu (also context and dropdown menus), Settings, DataTable, Dock, Tabs, Tree, VirtualList.
- Also in nav: AlertDialog, Clipboard, Focus Trap, HoverCard, Input Group, Plot, Shimmer, Table.

Mapping to your list:

| Need | Kit item |
|---|---|
| Sidebar | `Sidebar` |
| List / virtual list | `List`, `VirtualList`, `MessageScroller` |
| Markdown | `TextView` (Markdown and HTML) |
| Text input / editor | `Input`, `Textarea`, `Editor` (code editor with highlighting, gutter, folding) |
| Code highlighting | tree-sitter or `syntect` feature |
| Dialogs | `Dialog`, `AlertDialog`, `Sheet` |
| Popovers / menus | `Popover`, `HoverCard`, `Menu` |
| Tabs | `Tabs` |
| Avatars | `Avatar` |
| Theming | `Theme` (semantic themes, `themes/` dir, `.theme-schema.json`) |
| Icons | Lucide set via `gpui-kit-assets` |

**FACT.** Chat-specific parts exist in the source (https://github.com/longbridge/gpui-kit/tree/main/crates/component/src): `message.rs` (`MessageGroup`, `MessageAlignment`), `bubble.rs` (variants Filled, Secondary, Muted, Tinted, Outline, Ghost, Destructive; reactions), `attachment.rs` (status Pending, Uploading, Processing, Failed, ready), `message_scroller.rs` (virtual list with `FollowMode::Tail`, jump-to-bottom button, 400 px overdraw). Release notes for 0.8.0 add `MessageScrollerState::logical_scroll_top` and `TextareaState::set_submit_on_enter` "for message composers". Release notes: https://github.com/longbridge/gpui-kit/blob/main/release-notes.md

**UNVERIFIED.** What `Questionnaire` does. It may fit decision cards. Check the docs page before relying on it.

### 1.5 Maturity

- **FACT.** Version 0.x with weekly minor releases. The release notes list breaking changes (for example, crate rename `gpui-wry` to `gpui-webview` in 0.8.0).
- **FACT.** Kit pins GPUI exactly (`gpui-pre = "=0.3.10"`). Workspace comment says a newer snapshot once broke the build (issue #3156). Source: https://github.com/longbridge/gpui-kit/blob/main/Cargo.toml
- **FACT.** README claims AccessKit accessibility, headless UI integration tests, WASM showcases, 120 FPS, 200k-line editor.
- **INFERENCE.** Expect to upgrade every few weeks and to fix small breakage. One shipped commercial app is a good sign. It is one team's product, so gaps for chat UIs may remain.

### 1.6 Other candidates (not likely)

| Crate / repo | Facts |
|---|---|
| `gpui-ui-kit` 0.5.10 (https://github.com/pierreaubert/gpui-toolkit) | ISC license. Last release 2026-01-11. About 1k downloads. Small. |
| `gpui-markdown` 0.0.1 (https://github.com/AprilNEA/gpui-markdown) | MIT or Apache-2.0. Released 2026-08-25. 16 downloads. Single purpose. |
| `zeronsh/gpui-component` | A fork used by zeron. Not a separate product. |

---

## 2. GPUI itself

### 2.1 Distribution

- **FACT.** `gpui` on crates.io is 0.2.2, published 2025-10-22, Apache-2.0. https://crates.io/crates/gpui
- **FACT.** Zed `main` still says `gpui` version 0.2.2 (https://github.com/zed-industries/zed/blob/main/crates/gpui/Cargo.toml), but the Zed workspace sets `publish = false`. Crates `gpui_platform`, `gpui_tokio`, `gpui_web`, `gpui_ios`, `gpui_linux`, `gpui_wgpu` do not exist on crates.io (crates.io API returned "does not exist").
- **FACT.** Open issue "gpui: publish a new crates.io release (0.2.2 predates PinchEvent)": https://github.com/zed-industries/zed/issues/66275 (opened 2026-10-29).
- **FACT.** Workaround 1: git dependency on `zed-industries/zed` with a pinned `rev`. Workaround 2: `gpui-pre` snapshot crates, for example `gpui-pre` 0.3.10 (2026-10-26, "snapshot of zed@08cbb88"), `gpui-pre-platform`, `gpui-pre-web`. https://crates.io/crates/gpui-pre . The only crates.io owner of `gpui-pre` is `huacnlee` (the GPUI Kit maintainer). Zed does not publish it.
- **FACT.** Real apps use all three routes: zeron uses a git fork (`zeronsh/zui`), herdr-gpui and oxideterm use `gpui-pre`, GPUI Kit pins `gpui-pre`.
- **INFERENCE.** Supply-chain risk: `gpui-pre` has one owner and is a third-party republish. Prefer a git pin with a commit hash you review, or `gpui-kit`'s own pin, and vendor if you need audit control.

### 2.2 License

- **FACT.** The `gpui` crate is Apache-2.0 (Cargo.toml, `LICENSE-APACHE` in each GPUI crate directory). The Zed repo as a whole is mixed. Crates such as `markdown` and `auto_update` carry `LICENSE-GPL`. Do not copy those into an Apache or proprietary app.

### 2.3 Platforms

**FACT** (GPUI README https://github.com/zed-industries/zed/tree/main/crates/gpui , `gpui_platform` source):

- macOS: Metal. Needs the `font-kit` feature for glyphs.
- Windows: Win32 windowing, DirectWrite text. No features needed.
- Linux and FreeBSD: enable `wayland`, `x11`, or both. A `gpui_wgpu` renderer crate exists.
- README: "still pre-1.0" with frequent breaking changes. Docs are thin. Best docs are Zed source.
- Also in `main`: `gpui_ios`, `gpui_web`, OpenHarmony hook (see question 3).

### 2.4 Known gaps

| Area | Status |
|---|---|
| Accessibility | **FACT.** "gpui: Accesskit support" PR #56065 created 2026-05-07; `gpui` Cargo.toml depends on `accesskit`; later fixes (#57954 `Application::inaccessible()`, #64143 macOS adapter lifetime). **INFERENCE.** Young. Test with VoiceOver, Narrator, Orca before trusting it. |
| IME | **FACT.** API exists: `InputHandler`, `EntityInputHandler`, `marked_text_range`, `replace_and_mark_text_in_range`. Many open IME bugs in Oct 2026, for example #62764, #65868, #60578 (X11 IME breaks after XIMClientError), #65302 (Wayland preedit not distinguished). Source: https://github.com/zed-industries/zed/issues?q=IME |
| Text input widget | **FACT.** GPUI gives the input plumbing only. **INFERENCE.** No input element in `crates/gpui/src/elements` (div, img, list, uniform_list, svg, text, canvas, surface, anchored, deferred, animation). Use Kit `Input`/`Textarea`/`Editor`. |
| Web views | **FACT.** None in core. Open PR #62379 "native window composition support" (by huacnlee). Kit `gpui-webview` (Wry) is experimental: macOS, Windows, Linux X11/XWayland. https://gpui-kit.com/docs/comparison |
| Touch on Linux | **FACT.** Open issue #54473 "Zed ignores touch input on Linux". |

---

## 3. GPUI on mobile and web (short; user will not use it)

- **iOS. FACT.** `gpui_ios` merged into Zed `main` on 2026-10-14 (PR #63068 by Anthony-Eid). PR text: "experimental", "initial support, not full desktop or mobile feature parity", native edit menus and TLS workarounds deferred. Active follow-ups in Oct 2026 (#65806 back gesture, #66012 momentum). https://github.com/zed-industries/zed/pull/63068
- **Android. FACT.** Zed PR #61057 "Add Android platform support" was closed without merge on 2026-08-08. Community routes exist:
  - `itsbalamurali/gpui-mobile` (AGPL-3.0, 447 stars, pushed 2026-10-23): iOS (Metal via wgpu) and Android arm64 (Vulkan or GL ES). https://github.com/itsbalamurali/gpui-mobile
  - `tanlethanh/zedra` (MIT, 250 stars): remote control for AI coding agents, GPUI on iOS and Android through a Zed fork branch `feat/gpui-mobile` with a `gpui_android` crate. README links an App Store listing and a Google Play listing (not checked by me). https://github.com/deltaqdev/zedra
  - `Dylanmurzello/zed-android-port` (245 stars).
- **OpenHarmony. FACT.** PR #64579 merged 2026-10-26. `gpui_platform` expects an injected backend.
- **GPUI Kit. FACT.** Calls iOS "experimental". Says mobile and WASM "still need validation for the target application". Release notes mention Android Back-button handling and software keyboard options. https://gpui-kit.com/docs/comparison
- **Web. FACT.** `gpui_web` is in `main` (wasm, canvas, fetch-based HTTP client). Multithreaded mode "needs a nightly toolchain and a cross-origin-isolated page" (`gpui_platform/Cargo.toml` comment). Kit says wasm output is canvas based and "should not be treated as an HTML application by default". Open issues: #63740 keyboard on tap-away, #52715 wasm regression.
- **INFERENCE.** Mobile GPUI works in demos and at least one small shipped app. It is fork-based and young. Not a base for a production phone client now. The user already chose not to use it.

---

## 4. Chat-app building blocks

| Block | Source | Detail |
|---|---|---|
| Markdown rendering | **GPUI Kit** `TextView` | `TextView::markdown(id, src)` and `::html`. Selectable by default. Links open via `App::open_url` or `on_link_click`. Images: http(s) via app HTTP client, `data:` decoded, files need an `ImageCache`. `stream_fade(true)` fades streamed text. Limits: highlighting is opt-in, skipped on filled bubbles, thin table docs. https://gpui-kit.com/component/text-view |
| Markdown alternative | other crates | `pulldown-cmark` used by zeron (own `zeron-markdown` crate) and herdr-gpui. Zed's `markdown` crate is GPL. |
| Code block highlighting | **Kit** | tree-sitter (per-language features) or lighter `syntect` feature (0.8.0). tree-sitter wins if both on. herdr-gpui uses `syntect` 5.3 directly. |
| Virtualized timeline | **GPUI** + Kit | GPUI `list` and `uniform_list` elements. Kit `MessageScroller` wraps `ListState` with tail follow, prepend, splice, jump button. |
| Multi-line input | **Kit** | `Textarea`/`Input`/`Editor`. Enter-to-send via `set_submit_on_enter`. IME caveats from section 2.4. |
| Image display | **GPUI** `img` + Kit `Image` | Remote URLs need an HTTP client on the `Application`. Kit pins `gpui-pre-reqwest-client`. |
| Drag-and-drop files | **GPUI** | `ExternalPaths`, `FileDropEvent`, `div().on_drop::<T>()`, `can_drop`. File picker via `Platform::prompt_for_paths`. Kit `Attachment` shows upload state. |
| System tray | **other crate** | **FACT.** No tray API found in GPUI `platform.rs`. `tray-icon` 0.26.1 (2026-10-07, MIT or Apache-2.0). **INFERENCE.** It needs its own event loop on Linux (GTK). Prototype before committing. No GPUI app in my sample used it. |
| OS notifications | **GPUI** | **FACT.** `Platform::show_system_notification(SystemNotification)` with tag, action buttons, `on_system_notification_response`. No-op where unsupported. Alternative: `notify-rust` 4.18.2 (oxideterm uses it). Kit `Notification` is in-app toasts only. https://github.com/zed-industries/zed/blob/main/crates/gpui/src/platform.rs |
| Global hotkeys | **other crate** | **FACT.** Not in GPUI platform trait. `global-hotkey` 0.9.0 (2026-10-16). Same event-loop caveat as tray. |
| Auto-update | **other crate** | **FACT.** Not in GPUI. Zed's `auto_update` is GPL. Options: `self_update` 1.3.3 (2026-10-27), `velopack` 1.2.161 (2026-09-29), `cargo-packager` 0.11.8 (last release 2025-11-27). zeron says "The app updates itself"; I did not check how. |
| Voice | **Kit** `Speech` + crates | **FACT.** `speech` feature. System recognizer on macOS and Windows only. Linux needs your own `SpeechRecognizer`. `AudioInput` is replaceable. `cpal` 0.18.2 for custom capture. |
| Dock/panels (Tasks panel, thread side panel) | **Kit** | `Dock`, `Resizable`, `Sheet`, `Tabs`, `Sidebar`. |
| Decision cards, task-thread cards | **you build** | **INFERENCE.** Compose from `Bubble`, `Button`, `Tag`, `Progress`. No ready component. |

---

## 5. Async and networking

### 5.1 GPUI executors

**FACT** (https://github.com/zed-industries/zed/blob/main/crates/gpui/src/executor.rs):

- `ForegroundExecutor`: runs tasks on the main thread.
- `BackgroundExecutor::spawn(future)`: runs on a background thread; future must be `Send + 'static`. Also `spawn_with_priority`, `scoped`, `timer`.
- GPUI is not built on Tokio. It has its own scheduler and test executors with fake time.

### 5.2 Tokio next to GPUI

**FACT** (https://github.com/zed-industries/zed/blob/main/crates/gpui_tokio/src/gpui_tokio.rs):

- `gpui_tokio::init(cx)` makes a Tokio runtime with 2 worker threads. `init_from_handle(cx, handle)` uses your runtime.
- `Tokio::spawn(cx, fut) -> Task<Result<R, JoinError>>` runs a `Send` future on Tokio and returns a GPUI task. "The Tokio task will be cancelled if the GPUI task is dropped". `spawn_result` flattens `anyhow::Result`. `Tokio::handle(cx)` gives the handle.
- The crate is unpublished (`publish = false` workspace). Use a git dep or the fork route.
- **FACT.** Zed's `reqwest_client` crate keeps a global `OnceLock` runtime. If none is current, it creates one.

### 5.3 Working example

**FACT** (https://github.com/zeronsh/zeron/blob/main/Cargo.toml): zeron depends on `gpui`, `gpui_platform`, `gpui_tokio` (all from a git fork), `gpui-base`, `tokio` (full), `tokio-util`, `tokio-tungstenite 0.24` (rustls), `reqwest 0.12` (rustls, json, stream). This is the closest match to your hub-and-worker design.

Latest versions today: `tokio-tungstenite` 0.30.0 (2026-07-11), `reqwest` 0.13.5 (2026-09-08), `tokio` 1.54.1 (2026-10-21).

### 5.4 Pattern and pitfalls

- **INFERENCE (pattern).** Keep the WebSocket and HTTP code in a client core on Tokio. Send events to the UI over a channel. Read the channel in a GPUI `cx.spawn` task and call `entity.update(...)`. Never hold GPUI handles inside Tokio futures. Never await a Tokio-only future (tokio-tungstenite, reqwest) on a GPUI executor. It has no Tokio reactor.
- **FACT.** Dropping a GPUI task cancels the Tokio task. Keep the task handle (store it in the entity or call `.detach()`), or the connection ends silently.
- **FACT.** `gpui_tokio` default is 2 threads. Pass your own runtime for heavy workloads.
- **FACT.** Kit issue #2504 "Replace tokio sleep with background executor timer": use GPUI timers inside UI code.
- **INFERENCE.** Keep one Tokio major version across `gpui_tokio` and your deps. `reqwest` 0.13 vs the Kit-pinned `gpui-pre-reqwest` 0.12.15 can mean two HTTP stacks in one binary.
- **INFERENCE.** Putting the connection, auth, SQLite cache, and reducers in a separate crate with no GPUI dependency lets the desktop app and the mobile apps share it (see question 7).

---

## 6. Real apps built on GPUI (outside Zed)

Stars and push dates from GitHub API on 2026-10-29.

| App | Link | What it shows |
|---|---|---|
| Longbridge Pro | https://longbridge.com/desktop | Closed commercial trading desktop app on GPUI Kit. |
| zeron | https://github.com/zeronsh/zeron (3.4k stars, MIT) | Native control plane for Claude Code, Codex, Cursor and others. macOS, Windows, Linux app. Linux headless daemon. Optional multi-device sync. Closest analog. |
| waku | https://github.com/egoist/waku (1.6k, GPL-3.0) | Native app for coding agents. Uses a GPUI fork with webview support. |
| herdr-gpui | https://github.com/penso/herdr-gpui (1.8k, Apache-2.0) | Native client of a local daemon: terminal sessions, worktrees, agent activity. Uses `gpui-pre` 0.3.6, pulldown-cmark, syntect. |
| oxideterm | https://github.com/AnalyseDeCircuit/oxideterm (1.7k, GPL-3.0) | Terminal and SSH workspace with AI. Vendors a GPUI copy. |
| diri | https://github.com/cristicretu/diri (478, AGPL-3.0) | Many coding agents in parallel in one UI. |
| OpenLogi | https://github.com/AprilNEA/OpenLogi (24.5k, Apache-2.0) | Device config tool. Large user base. |
| sonora, hummingbird | https://github.com/sonorahq/sonora , https://github.com/hummingbird-player/hummingbird | Music clients. |
| zedis, dbdelve, dbflux, tusk | e.g. https://github.com/vicanso/zedis | Database and Redis GUIs. Data-heavy lists. |
| velotype | https://github.com/manyougz/velotype | Markdown editor. |
| Loungy | https://github.com/MatthiasGrandl/Loungy (MIT) | Launcher. Last push 2025-10-05, so stale. |
| Zedra | https://github.com/deltaqdev/zedra | GPUI on phones (see question 3). |

**INFERENCE.** At least four apps build agent-control UIs with live streams on GPUI. Chat plus markdown plus long lists is a proven shape. Most are small teams or single authors. Many use GPL or AGPL, so you cannot copy code into a closed-source app.

---

## 7. Shared Rust client core for mobile (Expo or native Swift/Kotlin)

Goal: share connection, auth, local SQLite cache, event reducers, and view state across desktop (GPUI), Expo, and native apps.

### 7.0 Shape of the core (INFERENCE)

- A `client-core` crate with no GPUI and no UI dependency. It owns: WebSocket and HTTP to the hub, auth tokens, SQLite cache, event reducers, and a view-state store.
- The UI reads state and sends commands. Desktop uses it directly in Rust. Mobile uses it through generated bindings.
- Keep async and runtime ownership inside the core. Expose small, coarse methods and listener callbacks. Avoid many fine-grained calls across the FFI.

### 7a. UniFFI (https://github.com/mozilla/uniffi-rs)

- **FACT.** `uniffi` 0.32.2, released 2026-09-23. MPL-2.0. 5,072 stars. Pushed 2026-10-28. Pre-1.0, so expect breaking changes between minor versions. https://crates.io/crates/uniffi
- **FACT.** Targets Swift and Kotlin (plus others). Changelog adds an experimental JNI-based Kotlin generator (`uniffi-bindgen-kotlin-jni`, "use at your own risk") and `uniffi_parse_rs`. https://github.com/mozilla/uniffi-rs/blob/main/CHANGELOG.md
- **Async. FACT.** Rust `async fn` can be exported with `#[uniffi::export]`. The foreign language drives the future with its own executor. "There's no requirement for a Rust event loop." Async trait methods work with `#[uniffi::export]` placed outside `#[async_trait::async_trait]`. **Cancellation is not supported directly**: "We don't directly support cancellation in UniFFI". Build your own `cancel()` flag. https://mozilla.github.io/uniffi-rs/latest/futures.html
- **Tokio. FACT.** The async internals page says tokio integration is possible: keep a runtime reference and bridge with `Runtime::spawn`. The `uniffi::async_runtime` attribute exists, but maintainers are unsure they will keep supporting it (issue #1726). It also warns that crates like `reqwest` silently start a Tokio runtime. https://github.com/mozilla/uniffi-rs/blob/main/docs/manual/src/internals/async-overview.md . **INFERENCE.** Own one global Tokio runtime inside the core. Do not rely on the attribute.
- **Callbacks. FACT.** Foreign implementations of Rust traits use `#[uniffi::export(with_foreign)]` (seen in ubrn docs and matrix-sdk-ffi). The manual page I tried (`foreign-traits.html`) returned 404, so I did not read the official page for thread-safety rules. matrix-sdk-ffi declares listener traits as `SyncOutsideWasm + SendOutsideWasm`, meaning `Send + Sync` on native.
- **Observable state pattern. INFERENCE** from matrix-sdk-ffi: Rust exposes `subscribe(listener)` that takes a `with_foreign` listener trait and returns a `TaskHandle`. Rust calls `listener.on_update(diff)` from its own thread. The native side hops to the main thread and updates SwiftUI, Compose, or a JS store. Send diffs or versioned snapshots, not one callback per field.

### 7b. uniffi-bindgen-react-native (ubrn)

Repo: https://github.com/jhugman/uniffi-bindgen-react-native

- **FACT.** npm `uniffi-bindgen-react-native` latest tag is `0.31.0-6`, published 2026-09-25. That is a prerelease label on the `latest` tag. 562 stars. Last push 2026-10-27. Repo license shows NOASSERTION on GitHub (I did not read the LICENSE file). Follows UniFFI 0.31.x per its version, while UniFFI itself is at 0.32.2.
- **FACT.** Generates TypeScript bindings. For React Native it builds a Turbo Module with JSI C++. Also supports WASM for web and N-API for Node. Supports sync calls, async calls JS to Rust, same-thread and async callbacks Rust to JS, objects by reference, records by value, enums.
- **Expo. FACT/UNVERIFIED.** The docs tree has `getting-started`, `pre-installation`, `troubleshooting`. I found no Expo guide and no Expo config plugin in the repo. Getting-started was "tested with versions 0.75 and 0.76". Issue #295 (Android crash on RN 0.80+ and Expo 54) was fixed and closed 2025-12-09. Issue #278 (iOS `TurboModuleRegistry.getEnforcing` failure) is still open. Issue #414 (version of `@ubjs/core` not tied to generator) is open. **INFERENCE.** It is a native module, so it needs an Expo development build or prebuild, not Expo Go. This is general Expo behavior, not read here. Plan a spike with your exact Expo SDK and New Architecture setting.
- **Users. FACT** (README): `@unomed/react-native-matrix-sdk` (matrix-rust-sdk in RN; npm 0.10.2, modified 2026-09-30), ChessTiles iOS, Whip iOS and Android, `@fressh/react-native-uniffi-russh`.
- **Maturity. INFERENCE.** Active and used in shipped apps, but prerelease-labelled, Expo is not a documented first-class path, and two version numbers must be kept in sync by hand. Medium risk. Native Swift/Kotlin bindings from UniFFI itself are the lower-risk path.

### 7c. Precedents

**matrix-rust-sdk with Element X** (https://github.com/matrix-org/matrix-rust-sdk)

- **FACT.** Apache-2.0, 2,342 stars, `matrix-sdk` 0.19.1 (2026-09-18). `bindings/matrix-sdk-ffi` uses UniFFI for native and wasm-bindgen for web. Cargo features include `sqlite`, `bundled-sqlite`, `indexeddb`. https://github.com/matrix-org/matrix-rust-sdk/tree/main/bindings/matrix-sdk-ffi
- **FACT.** Element X iOS is SwiftUI "on top of matrix-rust-sdk" (AGPL-3.0, release 26.10.8 on 2026-10-26). Element X Android is Jetpack Compose "using the Matrix Rust Sdk" (v26.10.0 on 2026-10-16). https://github.com/element-hq/element-x-ios , https://github.com/element-hq/element-x-android
- **FACT.** The FFI layer exposes many listener/delegate traits (`ClientDelegate`, `SyncNotificationListener`, etc.). `crates/matrix-sdk-ui/src/notification_client.rs` exists, and Element X iOS has an `NSE` directory (notification service extension).
- **INFERENCE.** In Rust: protocol, sync, crypto, storage, timeline logic, notification event fetch. In native: UI, navigation, push registration, OS integration. This is the closest model for your design. Also shows the same Rust core works in a React Native package via ubrn.

**Crux** (https://github.com/redbadger/crux)

- **FACT.** `crux_core` 0.20.0 (2026-08-07), Apache-2.0, 2,804 stars. README: "pre-1.0... production-ready, but occasional breaking changes". Design: side-effect-free core, thin UI shell, effects returned as values and run by the shell, generated types for Swift, Kotlin, TypeScript. Elm-like update/view loop. https://redbadger.github.io/crux
- **INFERENCE.** Good fit for "event reducers and view state" in one testable unit. Cost: you adopt its effect model. Your WebSocket and SQLite become shell capabilities, so the shell on each platform does I/O. That moves networking out of Rust, which differs from the Element X approach. Crux's published mobile path is Swift and Kotlin shells. I found no React Native shell in the README.

**Others**

- ubrn users above (Matrix RN SDK, russh, chess app).
- **UNVERIFIED (memory only).** Mozilla's application-services (Firefox mobile) is the origin of UniFFI. I did not check it this session.

### 7d. Push and background limits that shape the core

**FACT, iOS** (https://developer.apple.com/documentation/usernotifications/pushing-background-updates-to-your-app , read through Apple's JSON endpoint):

- Background (silent) notifications are low priority and "the system doesn't guarantee their delivery".
- The system "may throttle" them. Apple says not to send "more than two or three per hour".
- "If something force quits or kills the app, the system discards the held notification."
- The app "has 30 seconds" to finish background work.

**FACT, iOS notification service extension** (https://developer.apple.com/documentation/usernotifications/unnotificationserviceextension):

- It has "a limited amount of time". If you do not update content before time expires, the system shows the original content.
- You cannot modify silent notifications or notifications that only play a sound or badge.

**FACT, Android FCM** (https://firebase.google.com/docs/cloud-messaging/android/message-priority):

- In Doze, normal-priority delivery "may be delayed". High priority messages can wake a sleeping device.
- High priority is "meant for time sensitive, user visible content". FCM "uses 7 days of message behavior" to decide whether to deprioritize.
- For more processing time, use WorkManager or a foreground service.

**INFERENCE (design effects on the Rust core):**

1. A long-lived WebSocket cannot run in the background on iOS. Keep the live socket only while the app is in the foreground. On Android, a foreground service can hold it, but it needs a permanent notification and has store policy cost.
2. The hub must send push (APNs and FCM). Keep the payload small: an event id and a type. Show a generic text if the extension times out.
3. On iOS, a notification service extension is a separate process. The Rust core can be linked into it (Element X does this) but needs a small, fast code path: read token from the shared keychain, fetch one event over HTTPS, decrypt or format, return. Use a shared App Group container for SQLite, and be careful with concurrent writers.
4. On wake from push, run a "catch up since cursor" request. Do not rely on the push to carry state. Make reducers idempotent and keyed by event id and sequence.
5. In Expo, push registration and display use JS-side libraries, but custom NSE code is native. **UNVERIFIED** here: Expo's NSE and background-task support. Check `expo-notifications` and config plugins before choosing Expo.

### 7e. Trade-offs (no decision)

| Option | Maturity | Code sharing with Rust core | Effort |
|---|---|---|---|
| Native Swift + Kotlin with UniFFI | UniFFI 0.32.2, used by Element X (large app). Official path. | Highest. Core plus listener traits. UI written twice. | Highest UI effort, lowest binding risk. Two UI codebases. |
| Expo + ubrn | ubrn 0.31.0-6 prerelease, used in several shipped apps. Expo path not documented by ubrn. | Same core. One UI codebase in TypeScript. TS types generated. | Medium UI effort. Extra spike risk: dev build, New Architecture, version sync. |
| Crux with native shells | Crux 0.20.0, pre-1.0, production-ready claim. | Reducers and view state shared. I/O not shared. | Medium. Adopt its architecture; adds a rewrite of the core design. |
| Expo + pure JS client (no Rust core on phone) | Mature. | None. Re-implement protocol and reducers in TypeScript, keep a shared schema. | Lower binding risk. Logic is duplicated and can drift. |

### 7f. Dropped by request

Leptos, Dioxus, and Tauri mobile are out of scope. One verified fact is kept in case a web fallback comes back: iOS Web Push needs iOS 16.4 or later and the web app added to the Home Screen, and the permission request must follow a direct user action (https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/).

---

## Open items to check next

1. Read the Kit `Questionnaire` docs page. It may map to decision cards.
2. Prototype `tray-icon` and `global-hotkey` beside a GPUI event loop on Linux and macOS.
3. Spike ubrn on your exact Expo SDK with the New Architecture. Read the LICENSE file.
4. Read the official UniFFI foreign-traits page (correct URL) for thread-safety rules.
5. Verify Expo support for notification service extensions and background tasks.
6. Choose the GPUI pin strategy: git `rev` vs `gpui-kit` pin vs vendoring.
