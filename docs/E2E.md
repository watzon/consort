# End-to-end test, 2026-10-29

The lead ran the real binaries: the hub and a worker from `consort dev` on the Mac, the desktop app on the Mac, and a second worker on the Windows machine `bigboi`. The harnesses were the real Claude Code (through `claude-agent-acp`), Codex and OpenCode that are installed on each machine. The hub listened on the Mac's LAN address `192.168.188.138:7600` only during the test.

## Results against "Done means"

| # | Check | Result |
|---|---|---|
| 1 | `consort dev` starts a hub and a worker and prints a setup code | Pass |
| 2 | The app asks for the hub URL and the setup code, pairs, and opens Everything | Pass |
| 3 | A message in Everything starts a coordinator turn, and its posts show in the timeline | Pass, after fixes 1 to 3 below |
| 4 | The coordinator starts a task. The task runs, asks for an approval that the user answers in the app, goes to Ready for review, and the user resolves it | Pass |
| 5 | Create, rename and archive a channel. Its purpose reaches its coordinator | Pass. A Garden channel whose purpose said "end every answer with sprout" got an answer that ended with "sprout" |
| 6 | A second worker pairs with `consort worker pair`, and shows in the sidebar and in Settings > Workers with its accounts | Pass. `bigboi` paired with a code from Settings > Workers > Add worker |
| 7 | Spruce and Velvet, in Light and Dark, switch live | Pass |
| 8 | The worker and the app build and run on Windows. A Windows worker runs a task | Partly. Both build and run, and the hub placed a task on `bigboi`. The task failed because the Claude Code login on `bigboi` has expired, and Codex there is signed out. Sign in on `bigboi` to finish this check |
| 9 | `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` pass | Pass at each commit |

## Bugs that the test found

Fixed:

1. The coordinator's first hub tool call (`post_message`) opened a user approval, because Claude Code asks before it uses an MCP tool. The worker now allows hub tools itself (decision 108).
2. A coordinator approval or decision in a channel showed as plain text, with no buttons. The timeline now shows cards (decisions 103 to 107).
3. After a worker restart, the hub kept the old coordinator turn open, so new messages waited for ever. The worker now sends `reopened`, and the hub ends the old turn (decision 109).
4. After an app restart, the timeline showed only messages that came in live. The client now loads channel history, thread replies and older pages (decisions 110 to 113).
5. When a harness login expired, the probe still said "Signed in", and the task failed with only "Authentication required". A turn that fails for its login now marks the account signed out and says how to sign in (decision 114). The probe itself still cannot see an expired Claude Code login.
6. These desktop issues (decisions 115 to 122):

- The review card shows the whole review summary as plain text with blank gaps, and "Open thread" sits in the middle of it.
- A long approval summary (a file path) runs past the right edge of a task card.
- Resolve is only in the thread's "..." menu and the context menu.
- The thread panel says "Worktree of" for a task with no repo, and "The channel's default" for the harness after the task ran.
- After Create channel, the app does not open the new channel.
- In Settings > Workers, a long account detail (the Gemini CLI error) runs over the status column, and a detail that ends with a period gets a second one.
- The Add a worker dialog keeps "Waiting for the worker to connect" after the worker connects.
- The Add a worker dialog shows `curl -fsSL https://get.consort.dev | sh`. That installer does not exist yet.

Checked in the real app after the fixes: the history loads after an app restart, the message menu and the selected-text menu have the planned items, the message menu opens upward near the bottom edge, and Copy puts the selected text on the clipboard.

Open:

- Copy of a partial selection adds a newline at the end.

Not bugs:

- Tasks asked before they wrote a file because both channels use the Ask approval mode by default.
- The computer-use driver's `return` key did not reach the composer, but its `enter` key did. The app handles both.
