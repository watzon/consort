# Our own harness layer, with ACP as the common path

The worker drives every harness through our own Rust traits, `Harness` and `HarnessSession`. The first adapter speaks ACP. That one adapter covers Claude Code through `claude-agent-acp` (which runs the official Claude Agent SDK), OpenCode, and the ACP Registry agents. Native adapters for Codex (`codex app-server`) and Pi (`pi --mode rpc`) come next, where ACP loses features. T3 Code's provider layer is a design reference only, and we copy none of its code.

## Considered options

- Pi as the only harness. Rejected because the user wants to use each vendor's own harness, not one harness for every model.
- Fork T3 Code. Rejected: about 660,000 lines on Effect 4, no outside contributions accepted, and a per-machine data model that does not fit channels.
- Run T3 Code servers as workers. Rejected by the user: no dependency on T3 Code.
