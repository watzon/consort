# Paid tools come through Monid, behind the hub, and the user sets the limits

Monid is a catalog of paid tools with one wallet. The hub calls its API with a key that only the hub holds. A session uses Monid only through hub tools. By default each call asks the user. The user can loosen this as far as they want, down to no approvals and no limits. Each call shows what ran, what came back and what it cost.

A session can ask for a tool that its channel lacks: see ADR 0017.

## Context

- Composio connects the user's own accounts. Monid has no accounts to connect. The user adds money, and each tool call costs money.
- The user wants both of these to work: an agent that asks before each call, and an agent that runs all night with no approvals.
- An agent can call a tool many times, and text in a page or a file can steer a model. So the gate must not depend on the prompt (ADR 0011).
- Monid's catalog has thousands of tools. Their schemas cannot be in the context, and a list of checkboxes cannot show them all.
- Monid does not say whether a tool reads or changes. The read, change and delete rule of a connection has nothing to use here.

## Decision

Access to Monid:

- The hub calls `api.monid.ai` over HTTPS. The key is encrypted at rest and never leaves the hub, like the Composio key. Sessions do not get Monid's MCP server or its CLI.
- Monid has a "Use in" scope and channel exceptions, like a connection. The user chooses when they add the key. There is no default set of channels beyond what that dialog shows.
- By default a channel has all Monid tools. A channel can change to "Only these tools". The list holds whole providers, such as `apify`, and single tools. Each channel can also block a tool.
- The user fills these lists from an approval card ("Always allow this tool here", "Never use this tool"), and from the channel's recent calls. A search box adds more. The app never shows the whole catalog.

Tools for the session:

- `search_tools` searches the connections of the channel first, then Monid. It returns short rows with the source, the name, one line and the price. It leaves out tools that the channel may not use. A search is not a paid call and does not ask.
- `get_monid_tool` gives one schema, and `run_monid_tool` runs one tool. The hub checks access on each run.
- The prompt has a short block, only when the channel has Monid: when no connection can do the work, search before you say that you cannot.

When a call runs:

The hub takes the first step that matches:

1. The tool is blocked, or not on the channel's list. The hub refuses, and the error says where to change it.
2. The call is over a limit that the user set. The hub asks. It asks in every mode, also in Full access.
3. The session is in Full access. The call runs.
4. The channel's approval rule decides: ask every time (the default), ask once per tool in a task, or run without asking.

Spending:

- There are no limits by default. The user can set a limit per call, per task, per channel per month and per hub per month. A limit makes the hub ask. It does not end the work.
- The price on a card is an estimate from the tool's price and any limit field in the input. When the hub cannot estimate, the card says so. A limit is checked again after each call, with the real cost.
- The hub never tries a run again by itself, because Monid documents no idempotency and a second try can charge twice.
- A task card shows the Monid total of the task as it grows, so that a user sees runaway calls and can stop them.

Work with no user there:

- A routine and proactive turns each have one setting for Monid: off, ask or run without asking. A routine starts at ask, so its run waits in Needs input. Proactive turns start at off. The user can change each one.

What the user sees:

- Each call is one row in the chat or the task thread, which the hub writes. So it looks the same in every harness. It shows the tool, the status and the cost. It opens to show the full input, the full output and the cost details. All of this text is selectable.
- A file in the output goes to the channel's files, and the row has it as an attachment.
- The hub keeps the input and the output of each call with the row. This differs from a connector audit record, which keeps only a hash. Here the user asked to see them, and the data stays on the hub.
- Settings > Connectors shows the key, the balance, the spend per channel and a link to add money on Monid's site. Monid has no API to add money.

## Considered options

- Bundle the Monid CLI. Rejected because it needs Node.js and puts the key on another machine.
- Give sessions Monid's MCP server. Rejected because a model could then spend with no gate and no record, for the same reason that ADR 0011 gives for Composio.
- Fixed default limits. Rejected by the owner. A limit is the user's choice, and the default of asking at each call already protects a new user.
- A separate spending permission that Full access does not include. Rejected by the owner, who wants Full access to mean no approvals. Limits that the user sets still ask.
- A list of all tools with a switch for each. Rejected because the catalog is too large.

## Consequences

- With Full access or "run without asking" and no limits, only the Monid balance and Monid's own workspace budget stop the spending. A user who gives a channel Full access for its shell also gives it spending. The dialog for Full access must say so.
- At the default, a task that calls Monid 100 times makes 100 cards, unless the user picks "for this task" on the first card.
- Monid and its providers see the input of each call. A channel's private text can reach them when the user allows the call.
- Monid's stored input and output grow the hub's database. Large output goes to blobs.
- An overrun is possible with a limit, because a per-result tool can return more than the estimate.

## What real calls showed

Checked on 2026-10-31 with the owner's key and CLI 0.1.8. The documents differ from the API in places, and these results win:

- `cost.value` in `GET /v1/runs/{id}` is the charge. One image cost $0.0035, and the balance fell by exactly that, some seconds after the run ended. So a limit uses the cost of the run, not the balance. The `billing` object of the documents did not appear.
- `discover` and `inspect` cost nothing. A request with bad input gets HTTP 400, with no run and no charge. A provider error can be a `COMPLETED` run with no charge.
- `POST /v1/run` takes `input` with `body`, `queryParams` and `pathParams` (from the CLI's source). The run keeps the same `input`.
- A run usually starts as `RUNNING`, even when it takes 20 seconds, so the hub polls. `inspect` gives `metrics.runTimeMs`, which can set the wait.
- Prices have more shapes than the documents say: `PER_CALL`, `PER_RESULT`, `TIERED` and `PER_UNIT_MATRIX`. Some selectors read the output. So the hub can estimate a price only for some tools, and a card often shows the rate, not a total.
- `discover` gives `description` and an empty `summary`. `inspect` gives both. The name of a tool comes from the summary, or else from the description.
- `tags` has `verified`. `inspect` also has `categories`, such as `image-generation`. `discover` does not have `categories`, so access lists use providers and tools.
- A file output differs per tool. The image was a plain `http` address that expires in 24 hours, with the whole storage path encoded in its last part. Audio stays with Monid for 7 days, with a signed link for about 1 hour. So the hub downloads a file when the run ends, and writes it to the channel's files.

Not run: a tool with query or path parameters.
