# How Claude Desktop dictates

Read on 2026-10-30 from Claude Desktop 4.4212.0 on macOS and Claude Code 2.1.314. The sources are the app bundle (`swift_addon.node`, `app.asar`), the cached claude.ai web code, and the Claude Code binary. Client code cannot show what the server does, so each section marks server behavior as unknown.

## Answer

- Dictation is a cloud stream, not a bundled model. The bundle has no speech model, no Whisper and no Parakeet.
- The composer asks for Deepgram Nova 3. The web code sets `sttProvider:"deepgram-nova3"` and `useConversationEngine:true` in its call to connect.
- The client has no separate fixing pass. The text changes because each new transcript frame replaces the current phrase. When dictation ends, the client removes the gray mark.
- Unknown: whether the server runs anything after Deepgram. The name `use_conversation_engine` is Anthropic's parameter, and its meaning is not visible from the client.

## The stream

The socket is `wss://<host>/api/ws/speech_to_text/voice_stream`.

| Part | Value |
|---|---|
| Audio | `linear16` (the composer can also ask for `opus` when the conversation engine is off), 16 kHz, mono |
| Segments | `endpointing_ms=300`, `utterance_end_ms=1000` |
| Other parameters | `language`, `language_pinned`, `use_conversation_engine`, `forward_interims=typed` (behind a flag), `stt_provider`, `client_platform`, `conversation_uuid`, `no_capture`, `keepalive_ack` |
| Vocabulary | The header `x-config-keyterms`, comma-separated, at most 1024 characters in Claude Code |
| Client frames | Binary audio, `{"type":"KeepAlive"}` every 8 s in Claude Code, `{"type":"CloseStream"}` |
| Server frames | `TranscriptInterim`, `TranscriptText`, `TranscriptEndpoint`, `TranscriptError`, `KeepAliveAck`, `error` |

A second mode, `think_aloud`, sets `stt_provider=nova3` and adds `think_aloud_model`. The server then streams `EchoStart`, `EchoDelta` and `EchoDone`. The composer's call does not turn it on, and no caller appeared in the cached code. Some chunks were not in the cache, so a caller can exist elsewhere.

The native addon (`ClaudeAiSpeechSession`) uses the same socket for Quick Entry. It captures audio with `AVAudioEngine` and uses a cookie header. The `NSSpeechRecognitionUsageDescription` key in `Info.plist` suggests Apple speech recognition, but the addon has no `SFSpeech` or `SpeechAnalyzer` strings.

## The composer

- `TranscriptInterim` and `TranscriptText` use one handler. Each replaces the current phrase (`setInterimTranscript`).
- `TranscriptEndpoint` adds the current phrase to the finished transcript (`appendTranscript`).
- The text goes in as three parts: what was before the cursor, the dictated words, and what was after the cursor. So a draft survives on both sides.
- The dictated words carry a TipTap mark named `dictation`, with the classes `italic text-muted-as-secondary`.
- The mark stays until dictation ends, and then it is removed from the whole document. There is no accept step.
- Audio from before the socket opens is buffered and sent on open (`pendingAudio`, `preOpenAudioBytes`), so the first words are not lost.
- Stop sends `CloseStream` and waits for the last words (a drain). Claude Code waits up to 5 s, or 1.5 s if no data comes. If the wait ends first, the last interim text becomes final.
- Send during the drain is held. The notice says: "Dictation is still finishing, so your message wasn't sent. You can send again once it's done, or send now without the words still to come."
- States: `idle`, `starting`, `listening`, `finishing`. Stop reasons include `stopped`, `time_limit`, `not_granted`, `rate_limited` and `upstream_error`.

## What Consort can reuse

Consort must not use this endpoint. It is private to Anthropic's clients and needs the user's Claude session or OAuth token, and Consort never reads harness credentials. Reuse the design: replace the current phrase, commit at an endpoint, keep early audio, hold send during the drain, and mark the words until the end.

## Engines for Consort

The user chose on 2026-10-30: see [DICTATION-PLAN.md](../DICTATION-PLAN.md). This table is the research before that choice.

These numbers are from a web search, and most sources are vendors or a single benchmark. Measure them before choosing.

| Engine | Live text | Platforms | Note |
|---|---|---|---|
| Deepgram Nova 3, the user's key | Yes | All | The same model as Claude. Audio leaves the machine, and each minute costs money |
| Apple SpeechAnalyzer | Yes, it has volatile and final results | macOS 26 and later | A Swift API, so it needs a helper process, because Consort forbids `unsafe` |
| Parakeet TDT v3 | Batch, so it fits the fixing pass | All, through ONNX | About 25 European languages. No Chinese, Japanese, Korean, Arabic or Hindi |
| A streaming Parakeet or Moonshine model | Yes | All, through ONNX | Lower accuracy expected than batch models. Not tested here |

Sources: [independent SpeechAnalyzer benchmark](https://dev.to/iravoice/apple-speechanalyzer-vs-whispercpp-a-40-speaker-mac-benchmark-40i4), [vendor note on Parakeet v3](https://whispernotes.app/zh/blog/parakeet-v3-default-mac-model).
