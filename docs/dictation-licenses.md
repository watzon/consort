# Dictation licenses and credits

Consort has the MIT license. Dictation also uses programs and models with other licenses. Consort does not link the GPL programs. It starts them as separate programs and talks to them through their protocols.

## Programs

| Program | License | Source |
|---|---|---|
| `sayso-engine` (the speech engine) | GPL-3.0-only | <https://github.com/watzon/sayso> |
| `sayso-cleanup` (the cleanup runtime) | GPL-3.0-only | <https://github.com/watzon/sayso> |

A copy of the GPL comes with each release file that has these programs. You can get the source of the same version from the Sayso repository.

The Rust crates that Consort links (`sayso-sdk`, `sayso-speech`, `sayso-capture`, `sayso-protocol`, `sayso-engine-sdk`, `sayso-cleanup-models`, `sayso-cleanup-client`) have the MIT license.

## Models

Consort downloads a model only after you ask for it. The models stay on your computer.

| Model | By | License | Use |
|---|---|---|---|
| Parakeet TDT v3 | NVIDIA | CC BY 4.0 | Final text |
| Parakeet EOU 120M | NVIDIA | See its model card | Live preview in English on a Mac |
| Nemotron Multilingual | NVIDIA | See its model card | Live preview in other languages on a Mac |
| Silero VAD | Silero | MIT | Stop after silence |
| Cleanup 1 | Spokenly | Apache 2.0. Its NOTICE stays with it | Fix the text |
| S1-mini | Superwhisper | Apache 2.0 with an added term: keep the name "S1-mini" by "Superwhisper" exactly, and keep LICENSE and NOTICE | Fix the text |

The LICENSE and NOTICE files of a cleanup model download with it and stay beside it.

## Deepgram

Deepgram is a service, not part of Consort. It runs only when you add your own key and choose it. The audio then goes to Deepgram under your own agreement with them.
