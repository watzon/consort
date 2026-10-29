# Build decisions to review

The user was away during the first build, from 2026-10-29. These are the choices made without them. Each one can be changed. Newest last.

| # | Area | Decision | Why | Easy to change? |
|---|---|---|---|---|
| 1 | Process | "Astra agents" for review was read as independent reviewer subagents, because no tool or agent named Astra exists on this machine. | Nothing matched the name | Yes |
| 2 | GPUI | Pin `gpui-kit = "=0.8.1"` from crates.io (it brings `gpui-pre = "=0.3.10"`). Do not use the local `watzon/gpui-kit` fork. | The fork is on a feature branch; the published release is the stable base | Yes |
| 3 | GPUI | The selected-text menu uses GPUI Kit's cross-platform context menu, not the native macOS text menu that Kyllene uses. | The native menu needs an `unsafe` exception, which was approved for Kyllene only. The GPUI Kit menu also works on Windows | Yes |
| 4 | GPUI | The "progressive blur" at the top and bottom of the chat is a layered gradient fade from the canvas color to transparent. | GPUI has no blur filter for elements | Yes, if GPUI gains a blur |
| 5 | Repo | Product binary is `consort`. The desktop app binary is `consort-desktop`. | Matches the product name | Yes |
