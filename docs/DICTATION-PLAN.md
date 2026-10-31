# Plan: live dictation on the Sayso SDK

Status: the user chose the direction on 2026-10-30. Built on 2026-10-30, not committed, with a path dependency on an uncommitted Sayso. Step 5 of "Work in Sayso" and the engines in the Windows and Linux release files are open. [DECISIONS 761](DECISIONS.md) replaces DECISIONS 738. Designs: [notes/design-dictation.md](notes/design-dictation.md). How Claude Desktop does it: [research/claude-desktop-dictation.md](research/claude-desktop-dictation.md). The Sayso design: `../sayso/docs/sdk.md`.

## What the user asked for

- Words appear in the composer while the user speaks. They are gray until dictation ends.
- On this device: Parakeet TDT v3 writes the text. A small cleanup model fixes the text as soon as the user stops.
- Optional: Deepgram with the user's own key.
- With both engines: a local voice activity detection (VAD) model stops dictation after a long silence.
- Sayso's engine is the back end. Consort does not build speech code again.

## Decisions

1. **Dictation stays on the client.** The microphone, the models and the audio are on the user's computer. No audio goes to the hub. The move of harnesses to the hub ([HUB-PLAN.md](HUB-PLAN.md)) does not change this.
2. **A new crate, `consort-dictation`.** It holds the state of one dictation and calls the SDK. It has no GPUI and no hub I/O. `consort-desktop` calls it. `consort-client` stays free of speech code, because mobile will not use this engine.
3. **Start with local mode.** Consort ships the engine and the cleanup runtime. Using a running Sayso (`Mode::Auto`) comes later. It needs the Host API approval and the Swift engine's `store_v1`.
4. **Live text.**
   - Windows and Linux: Parakeet TDT v3 is the preview and the final model. The Rust engine has `redecode_preview`, which decodes the growing recording again about every 600 ms.
   - macOS: the catalog gives Parakeet TDT v3 only the `final` role. The preview for English is Parakeet EOU 120M, which has no punctuation or capitals. Then the final pass replaces it, so the correction is visible, as the user likes.
   - macOS, other languages: Nemotron Multilingual is the first choice for the preview. Test it. If it is not good enough, port `redecode` to the Swift engine.
   - Downloads with Cleanup 1. Windows and Linux: 1.1 GB. macOS in English: 1.3 GB (222 MB more). macOS in other languages: 1.8 GB (664 MB more). A port of `redecode` removes this extra download on macOS, so port it if the size becomes a problem.
5. **Cleanup model: Cleanup 1 by default, and S1-mini as a choice.** Cleanup 1 knows 7 languages and takes the text around the cursor. S1-mini knows English only. In Sayso's evaluation Cleanup 1 passed 41 of 47 cases and S1-mini 41 of 51, on sets that differ. Median time is 82 ms against 68 ms on an M5 Pro. The file is 639 MB against 484 MB. S1-mini must always show as "S1-mini" by "Superwhisper".
6. **A failed cleanup never loses text.** A crash, a timeout, an unsupported language or a missing file inserts the text of the final pass.
7. **Auto-stop.** Silero VAD runs on the device with both engines. The default is 30 seconds with no speech, and the choices are 15 s, 30 s, 60 s and Never. Auto-stop ends dictation as Stop does. It never sends the message.
8. **Deepgram.** Sayso uses cloud models for the final role only, and its Deepgram client has no WebSocket. So Deepgram replaces Parakeet for the final pass of each part. The live preview stays local. A streaming Nova 3 client is later work, if the local preview is not good enough with Deepgram.
9. **The system dictation code goes away.** Remove `start_system_dictation` and the hint.
10. **The Deepgram key stays on this computer.** It never goes to the hub, because the audio does not either.
11. **The shortcut does two things.** A tap starts and stops. A hold of more than about 300 ms is push to talk. There is no setting.

## What the SDK has today

| Part | State |
|---|---|
| `sayso-engine-sdk`, `sayso-protocol` | MIT, on crates.io at 0.13.0 |
| `sayso-speech`: live sessions, parts, final pass, VAD | MIT, built, not published |
| `sayso-capture`: microphone, 16 kHz mono | MIT, built, not published |
| `sayso-sdk`: `Speech` with its modes | Not built (Sayso `docs/sdk.md`, step 12) |
| Model store `v1` | Rust engine done. Swift engine open (step 11) |
| Cleanup: `sayso-core::cleanup`, `sayso-cleanup-client` | GPL, in the app. Not in the SDK |
| Cleanup runtime `sayso-cleanup`, the engines | GPL. An app ships them as separate programs |
| Stop after silence | Not in `SessionSpec` |
| Deepgram, `sayso-transcribe` | GPL, HTTP only |

## Work in Sayso

Consort is MIT, so it cannot link GPL crates. Every author of the cleanup crates is Chris (`git log`, 2026-10-30), so the code can move to MIT, as ADR 0002 allows. Check again before the move.

1. Build `sayso-sdk` with `Mode::Local`: a live session from the microphone, `partial` and `segment` events, model download, and load and unload. Build the other modes after that. An engine without `store_v1` uses its own folder, so the Swift work does not block this.
2. Move the cleanup logic (`render`, `plan_chunks`, `check`, the catalog, the client, the download) to an MIT crate, and add it to the SDK.
3. Add a silence limit to `SessionSpec`, with one event when the limit stops the session.
4. Add a small MIT Deepgram client behind `SpeechEngine`.
5. Publish the crates. Until then, Consort pins a git commit.

## Work in Consort

1. `consort-dictation`: the states `idle`, `starting`, `listening`, `finishing`, and the three parts of the text (before the cursor, dictated, after the cursor). Tests use a fake engine.
2. Packaging: the engine and the cleanup runtime in each release file, their signing on macOS, `NSMicrophoneUsageDescription` and the audio entitlement in `scripts/bundle-macos.sh`, which has neither today. Add the licenses of the GPL programs and the models (Parakeet is CC BY 4.0, so it needs the credit).
3. The composer: gray text for a range. `TextareaState` cannot color a range, because GPUI Kit 0.8.1 gives text decorations only to `EditorState` (`create_decorations_collection`, with a byte range and a `HighlightStyle`). Move the composer and the reply box to `EditorState`, with line numbers, folding, indent guides and search off and soft wrap on. Test this first: the look, the context menu, IME, paste and undo. Each partial must not become its own undo step. A decoration does not grow at its edges, so set it again after each partial.
4. Settings > Dictation, the failure notices, and the shortcut.
5. Docs: replace DECISIONS 738, and the Dictation section of `design-system.md`.

## What the build changed in this plan

- **`EditorState` has no auto-grow and a minimum of 2 rows.** A wrapper, `ui::parts::message_field`, sizes it to its wrapped rows and clips it.
- **Plain edits make one undo step for each partial.** `set_value` clears the history. `insert` and `replace` are atomic steps. The IME route draws an underline. So a new partial dispatches `Undo` and then inserts the new text (`dictation.rs`). The stack holds one dictated step above the draft's history. Undo needs the box on screen. When it does not work, the write replaces the range.
- **Audio during Starting.** `CaptureMicrophone::open_early` starts the capture before the models load, and the session takes that feed first. Live sessions opened the microphone only after the load before this.
- **The engine has a resource bundle.** The macOS app has it in `Contents/Helpers/sayso-engine/`, with the bundle beside it, not in `Contents/MacOS`.
- **The silent model.** Silero VAD downloads with the speech models and has no row in the notice, because it is about 1 MB.
- **Cancel download** closes the notice and stops the next files. The file that downloads now finishes, because the engine protocol has no cancel.
- **The Settings download button** shows the size of what a click downloads. This is more than "483 MB" when the other models are missing too.
- **The download notice** has a second button, "Change engine", as V4 draws it.
- **A release needs Sayso.** The path dependency means that CI and `release.yml` cannot build Consort until Sayso is committed and published. Do step 5 before the next release.

## Tests that decide things

Measured on 2026-10-30 on this Mac, with recorded clips through the macOS engine, no microphone (`crates/consort-dictation/tests/real_models.rs`).

- **The preview on macOS for a language other than English: Nemotron Multilingual is good enough to keep for now.** A German clip from a synthetic voice gave the right words, with 10 word updates in about 15 s. English with Parakeet EOU 120M gave 30 updates in its clip. A real voice can still change this result. So the port of `redecode` stays a later choice.
- **Memory with the three models loaded: about 1.13 to 1.17 GB.** The engine is 169 to 204 MB and the cleanup runtime is 957 to 974 MB (resident size of each process). CoreML memory may not count in the engine's figure.
- **Load the cleanup model while the microphone records.** The first start of a model set takes 19 to 24 s to reach listening, not about 4 s, because the speech models compile too. A second start, in a new process, takes 287 ms. The fix came 235 to 473 ms after the stop, because the cleanup model was loaded. So the preload works, and only the first start needs "Getting the speech model ready". A session keeps the audio during that time (`open_early`).
