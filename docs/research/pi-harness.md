# Pi and Oh My Pi as the agent harness: research report

Date of research: 2026-10-28. Method: local installs (`pi` 1.2.0, `omp` 18.13.3), `--help` output, packaged docs and type definitions, GitHub API (`gh api`), and one web fetch of Anthropic's terms page. No model was prompted. No login was started. No credential file was read.

Path shorthand used below:

- `PI` = `/opt/homebrew/lib/node_modules/@earendil-works/pi-coding-agent` (installed Pi 1.2.0)
- `PI_DEPS` = `PI/node_modules/@earendil-works` (sub-packages that ship with it)
- `OMP_BIN` = `~/Library/Application Support/Tern/omp/versions/18.13.3` (installed omp)

Labels: **FACT** means a source states it or I observed it. **INFERENCE** means my conclusion from facts.

Important correction to the question text: the repo `badlogic/pi-mono` no longer exists under that name. GitHub redirects it to `earendil-works/pi`. The npm scope is now `@earendil-works/*`. (FACT: `gh api repos/badlogic/pi-mono` returns `full_name: earendil-works/pi`; `PI/package.json` has `name: @earendil-works/pi-coding-agent` and `repository.url: git+https://github.com/earendil-works/pi.git`.) The installed Pi (1.2.0) is also not the latest. GitHub shows v1.4.0 released 2026-10-21 (FACT: `gh api repos/earendil-works/pi/releases`).

---

## 1. Package layout and licenses

**FACT** (sources: `PI/package.json`, `PI_DEPS/*/package.json`, `gh api repos/earendil-works/pi`):

| Package | Role (from its package.json description) | License |
|---|---|---|
| `@earendil-works/pi-ai` | Unified LLM API with automatic model discovery and provider configuration. Providers, auth, OAuth, streaming, cost tracking. | MIT |
| `@earendil-works/pi-agent-core` | General-purpose agent with transport abstraction, state management, attachment support. The agent loop. | MIT |
| `@earendil-works/pi-coding-agent` | Coding agent CLI with read, bash, edit, write tools and session management. Also the SDK, RPC mode, extensions, skills, MCP wiring. | MIT |
| `@earendil-works/pi-tui` | Terminal UI library with differential rendering. | MIT |
| `@earendil-works/pi-mcp` | Standalone MCP client "for pi and other applications". | MIT |
| `@earendil-works/pi-codemode` | Sandboxed JavaScript execution where the only capability is calling injected tools. | MIT |
| `@earendil-works/pi-telemetry` | Vendor-neutral telemetry contracts and typed schema utilities. | MIT |

- All seven packages are version 1.2.0 in the install, require Node `>=22.19.0`, and live in the one repo `earendil-works/pi` (directories `packages/ai`, `packages/agent`, `packages/coding-agent`, `packages/tui`, `packages/mcp`, `packages/codemode`, `packages/telemetry`).
- Repo: license MIT, 118,584 stars, 350 open issues, last push 2026-10-29 (`gh api repos/earendil-works/pi`).
- I found no web UI package in the installed dependency tree. I did not list the repo's `packages/` directory, so I cannot say whether a web UI package exists in the repo. Not verified.
- FACT: `PI/README.md` says "New issues and PRs from new contributors are closed automatically." A maintainer must reply `lgtm` to open the gate (bot text on issue `earendil-works/pi#9992`).

## 2. Embedding: SDK and subprocess

**FACT** (source: `PI/docs/sdk.md`, `PI/docs/rpc.md`, `PI/docs/cli-integration.md`, `PI/dist/core/sdk.d.ts`, `PI/dist/core/agent-session.d.ts`).

Pi has four integration modes: print mode, JSON event stream, RPC mode (`pi --mode rpc`, JSONL on stdin and stdout), and the in-process TypeScript SDK. The package exports `"."` (SDK) and `"./rpc-entry"`. `RpcClient` is exported for TypeScript hosts that drive Pi as a child process (`PI/docs/rpc.md`). The SDK doc says it embeds Pi "in a Node.js or Bun process".

### Create a session, prompt, stream, abort, queue

```ts
import { createAgentSession, SessionManager } from "@earendil-works/pi-coding-agent";

const { session } = await createAgentSession({
  cwd: "/path/to/workspace",
  sessionManager: SessionManager.inMemory(),   // or persistent default
  // model, thinkingLevel, scopedModels, tools, noTools, excludeTools,
  // customTools, resourceLoader, settingsManager, modelRuntime, agentDir
});

const off = session.subscribe((event) => {
  if (event.type === "message_update" && event.assistantMessageEvent.type === "text_delta")
    process.stdout.write(event.assistantMessageEvent.delta);
});

await session.prompt("Explain this repository");       // resolves when the run finishes
// While streaming, prompt() must say how to queue:
await session.prompt("Also check tests", { streamingBehavior: "followUp" }); // or "steer"
await session.abort();          // stops the run and waits for idle
await session.waitForIdle();
off(); session.dispose();
```

Source for the pieces: `CreateAgentSessionOptions` in `PI/dist/core/sdk.d.ts` (fields `cwd, agentDir, modelRuntime, model, thinkingLevel, scopedModels, noTools, tools, excludeTools, customTools: ToolDefinition[], resourceLoader, sessionManager, settingsManager`). `prompt(text, options?)`, `abort()`, `waitForIdle()`, `setModel(model)`, `compact(customInstructions?)`, `setAutoCompactionEnabled`, `getContextUsage()`, `getSessionStats()`, `getLastAssistantText()` are in `agent-session.d.ts`. `streamingBehavior?: "steer" | "followUp"` is in `PromptOptions` (`agent-session.d.ts` line 182). `steer()` and `followUp()` exist directly (`sdk.md`: they return `"queued"` or `"handled"`).

- **Steer** = message enters after the current assistant turn and its tool calls. **Follow-up** = message enters after the run finishes its pending work (`sdk.md`).
- Use `agent_settled`, not `agent_end`, to know that Pi will not continue by itself. `agent_end` only ends one low-level run (`sdk.md`, `rpc.md`).
- Custom system prompt: `new DefaultResourceLoader({ systemPromptOverride: () => "...", appendSystemPromptOverride: () => [] })` then `await loader.reload()` and pass `resourceLoader` (`PI/examples/sdk/03-custom-prompt.ts`).
- Custom tools: pass `customTools: ToolDefinition[]` or register with `pi.registerTool()` in an inline extension (`PI/examples/sdk/05-tools.ts`, `PI/docs/extensions.md`). A tool has a name, description, TypeBox schema, and `execute()`.
- Compaction: automatic near the context limit; `session.compact()` manual; extension hooks in `PI/docs/compaction.md` (463 lines). Compaction adds a summary and does not delete session entries (`PI/docs/sessions.md`).
- RPC example record: `{"id":"req-2","type":"prompt","message":"Review this repository"}` gives `{"type":"response","command":"prompt","success":true,"data":{"disposition":"started"}}`. Close stdin to shut down (`PI/docs/rpc.md`). All commands are listed in `PI/docs/rpc-commands.md` (860 lines).
- SDK sessions do NOT load the CLI's built-in `codemode`, `tool_search`, and MCP extensions. You add `createMcpExtension()` to `extensionFactories` of `DefaultResourceLoader` and call `session.bindExtensions()` (`sdk.md`, example `PI/examples/sdk/14-codemode-mcp.ts`).

**INFERENCE:** The SDK is the right embedding surface for a Node or Bun orchestrator. RPC is the right surface when a Pi process must run on another machine or in another language. Both exist and are documented as first-class.

## 3. Providers and auth

**FACT** (sources: `PI/docs/providers.md`, `PI_DEPS/pi-ai/README.md`, `PI_DEPS/pi-ai/dist/providers/*`, `PI_DEPS/pi-ai/dist/auth/oauth/*`).

Built-in provider list (pi-ai README "Supported Providers"): OpenAI, Ant Ling, Azure OpenAI, OpenAI Codex (legacy), Radius, TypeSafe, DeepSeek, NVIDIA NIM, Anthropic, Google, Vertex AI, Mistral, Groq, Cerebras, Cloudflare AI Gateway, Cloudflare Workers AI, xAI, OpenRouter, Vercel AI Gateway, ZAI Coding Plan, MiniMax, Together AI, Baseten, Hugging Face, Moonshot, GitHub Copilot, Amazon Bedrock, OpenCode Zen and Go, Fireworks, Kimi For Coding, Meta, Qwen Token Plan, Xiaomi MiMo, plus any OpenAI-compatible API (Ollama, vLLM, LM Studio) through `models.json`.

Your three must-haves:

| Need | Status | Detail and source |
|---|---|---|
| Anthropic API key | Built in | `ANTHROPIC_API_KEY` (`providers.md`). Also workload identity federation env vars. |
| Anthropic subscription OAuth | Built in, with a billing warning | "Anthropic (Claude Pro/Max subscription)" is in the OAuth list (pi-ai README, "OAuth Providers"). `ANTHROPIC_OAUTH_TOKEN` env var is also read. Pi shows: "Third-party harness usage draws from extra usage and is billed per token, not your Claude plan limits" (text quoted in issue `earendil-works/pi#3808`; the fix added setting `warnings.anthropicExtraUsage`). See section 4 for the terms. |
| OpenAI ChatGPT/Codex OAuth | Built in, two providers | (a) `openai` provider has "Sign in with ChatGPT: uses the ChatGPT subscription with the OpenAI API" (pi-ai README line 1744). Implementation `dist/auth/oauth/openai-chatgpt.js`: authorize URL `https://auth.openai.com/api/accounts/authorize`, token URL `https://auth.openai.com/api/accounts/oauth/token`, loopback callback `http://127.0.0.1:1455/auth/callback`, with a "paste the full callback URL" fallback for headless use. It needs a per-install device UUID. Error text says port 1455 may be held by "another pi session or by the Codex CLI". (b) `openai-codex` (legacy, "superseded by Sign in with ChatGPT") with its own `openai-codex-responses` API, device-code endpoints under `https://auth.openai.com/api/accounts/deviceauth/*`, and WebSocket/SSE transport choice (`dist/auth/oauth/openai-codex.js`, README line 1825). |
| Fireworks | Built in (not a custom provider) | `dist/providers/fireworks.js`: provider id `fireworks`, base URL `https://api.fireworks.ai/inference`, `FIREWORKS_API_KEY`, APIs `anthropic-messages` and `openai-completions`. |

**Where tokens live (FACT):** `<agent-dir>/auth.json`, default `~/.pi/agent/auth.json` (`PI/docs/configuration.md`, `providers.md`). Override the directory with `PI_CODING_AGENT_DIR` or the SDK `agentDir`. An API key in `auth.json` can be a shell command prefixed with `!` (`providers.md`). In code, pi-ai uses a `CredentialStore` interface (`read`, `list`, `modify`, `delete`). OAuth refresh runs inside `modify()` so "concurrent requests and processes cannot double-refresh a rotated token" (pi-ai README line 440). Apps can inject their own store. I did not read `auth.json`.

**Credential precedence (FACT, `models.md`):** runtime `--api-key`, then `auth.json`, then `models.json` `apiKey`, then env vars or ambient cloud credentials.

**Mid-conversation model switch across providers (FACT):** Yes. pi-ai README "Cross-Provider Handoffs": thinking blocks from another provider become `<thinking>`-tagged text; tool calls, results, and text pass through. `AgentSession.setModel()`, `cycleModel()` exist; "A session records model and thinking-level changes" (`models.md`). Caveat (FACT): README says thinking blocks from another provider are converted to text. INFERENCE: provider-specific reasoning state (signatures, encrypted reasoning) is therefore not reused after a switch.

## 4. Claude Agent SDK integration

**FACT:**

- No official Pi provider runs the Claude Agent SDK or the `claude` CLI. The official Anthropic path in Pi is direct Messages API with an API key or OAuth.
- Community: `prateekmedia/claude-agent-sdk-pi` ("Claude agent sdk as a provider for pi", 117 stars, last push 2026-05-16, npm `claude-agent-sdk-pi`). Its README: it "registers a custom provider that routes LLM calls through the Claude Agent SDK while pi executes tools"; "Tool execution is blocked in Claude Code; pi executes tools natively"; built-in tool calls are mapped to Claude Code tool names; custom tools reach Claude Code "via in-process MCP"; provider id `claude-agent-sdk`; auth is Claude Code login or `ANTHROPIC_API_KEY`. Source: `gh api repos/prateekmedia/claude-agent-sdk-pi/readme`.
- A second design exists as an issue: `earendil-works/pi#9992` "Add opt-in Claude Agent SDK carrier for Pi-owned tools". It has the SDK return exactly one structured proposed Pi tool per response, and Pi executes it. The author lists failure modes that the design must "fail closed" on: non-empty resume, model switch, abort, context or tool changes, images, unsupported compaction and branch summaries. The bot auto-closed the issue (new-contributor gate), so it is not an accepted feature.
- Pi's provider contract (`PI/docs/custom-provider.md`): a provider implements `streamSimple` that emits `start`, balanced text/thinking/toolcall events, and one terminal `done` or `error`. Tool calls come out of the provider as content blocks. `pi-agent-core` runs the tools and calls the provider again (`PI_DEPS/pi-agent-core/README.md`: `streamFn`, `beforeToolCall`, `afterToolCall`, steering and follow-up queues).

**INFERENCE (difficulty):** Pi's loop owns tool execution, queues, retry, and compaction. The Claude Agent SDK also owns a loop with its own tools, sessions, hooks, and subagents. Putting the SDK behind `streamSimple` means you must turn the SDK into a one-step "propose a tool call" model call, deny the SDK's own tool execution, and translate tool names. You lose the SDK's native sessions, subagents, skills, and hooks. Abort, resume, model switch, images, and compaction are the weak spots (the same list as issue #9992). It works as a demo and the community package proves it. I rate it medium risk for production. Cleaner option: run the Claude Agent SDK as its own harness behind your own runtime interface, next to a Pi-based runtime for OpenAI and Fireworks. Then each provider keeps its native loop.

**Terms constraint (FACT, primary source `https://code.claude.com/docs/en/legal-and-compliance`, fetched 2026-10-28):** "Developers building products or services that interact with Claude's capabilities, including those using the Agent SDK, should use API key authentication through Claude Console or a supported cloud provider. Anthropic does not permit third-party developers to offer Claude.ai login into their own applications, or to route requests through Free, Pro, or Max plan credentials on behalf of their users." The page also says an end user may sign in to "the unmodified Claude Code binary with their own Claude subscription." Secondary news (not verified against an Anthropic primary source): since 2026-04-04 subscription limits do not cover third-party harnesses; they bill as extra usage (search results, e.g. `https://gigazine.net/gsc_news/en/20260220-anthropic-third-party-block`). Pi's own warning matches the extra-usage claim.

**INFERENCE:** Do not ship Pi's built-in Anthropic subscription OAuth inside your product. Offer Anthropic by (a) API key, or (b) the Claude Agent SDK using the user's own Claude Code sign-in on their own device. This also fits your device model: the sign-in stays on each user device.

## 5. Extensions, hooks, skills, MCP, templates, themes

**FACT** (sources: `PI/docs/extensions.md`, `skills.md`, `prompt-templates.md`, `themes.md`, `packages.md`, `mcp.md`):

- **Extensions:** TypeScript modules (`jiti` is a dependency in `PI/package.json`; INFERENCE: it loads them). They use `pi.on(event, handler)`, `pi.registerTool()`, `pi.registerProvider()`, commands, flags, UI widgets. Events include `before_agent_start`, `context`, `context_with_system`, `message_end` (can replace the message), `tool_call` (can mutate input or block), `tool_result`, `turn_end`, `agent_before_settle` (can return `continue: true`), `provider_stream_event`, `user_bash`, `project_trust`, `cache_warming_decision`. Inline extension factories work in SDK sessions.
- **Skills:** the Agent Skills spec, a directory with `SKILL.md` and frontmatter `name` and `description`; discovery from `~/.pi/agent/skills/`, `.pi/skills/`, and packages. Descriptions are advertised; the body loads on demand.
- **Prompt templates:** files in `prompts/` become slash commands. **Themes:** JSON files with a schema (`PI/schemas/theme.schema.json`).
- **Pi packages:** bundles of the above, installed from npm or git (`pi install npm:...`).
- **MCP is built in as of 1.x** (the old "Pi has no MCP" claim is out of date). `pi-mcp` is a standalone client. Transports: stdio and streamable HTTP; SSE is rejected. Config: `~/.pi/agent/mcp.json` and project `.pi/mcp.json` (project file read only after trust). Format matches Claude Desktop and Claude Code `mcpServers`. OAuth for remote servers with tokens in `~/.pi/agent/mcp-auth.json`. Headers and env support `${VAR}` and `!command`. Tools are named `mcp__<server>__<tool>`. Exposure modes: direct, `codemode`, deferred (`tool_search`). Commands: `pi mcp add|remove|list|login|logout`. Source: `PI/docs/mcp.md`; the 1.0.0 entry in `PI/CHANGELOG.md` shows MCP OAuth hardening.
- **Codemode:** the model writes JS that calls tools in a sandbox (`pi-codemode`), also reaches classifier and image models (`PI/docs/codemode.md`).

**INFERENCE:** LAN MCP servers fit: use streamable HTTP with headers, or stdio on the device that runs the Pi process. Because `mcp.json` is per agent dir and per project, a per-device Pi process naturally sees that device's MCP servers.

## 6. Sessions

**FACT** (`PI/docs/session-format.md`, `sessions.md`, `sdk.md`):

- JSONL, one JSON object per line. First line is a header `{"type":"session","version":3,"id":...,"cwd":...,"parentSession"?}`. Later entries have `id` and `parentId`, so the file is a tree. Branching moves the active leaf and never deletes branches.
- Path: `~/.pi/agent/sessions/--<path>--/<timestamp>_<session-id>.jsonl`. Change with `--session-dir`, `PI_CODING_AGENT_SESSION_DIR`, `sessionDir` setting, or `agentDir`. Custom session id through the SDK or `--session-id`.
- Version 3 is current; older files migrate on load. System prompt and tool loadout are persisted as `system` messages that patch named `sections` and list `toolsAdded` and `toolsRemoved`.
- Resume: `--continue`, `--resume`, `--session <path|id>`, `--fork`. In the SDK: `SessionManager` (persistent or `inMemory()`), `AgentSessionRuntime` with `newSession()`, `switchSession()`, `fork()`, `importFromJsonl()`. Branch commands: `/tree`, `/fork`, `/clone`. `exportToJsonl()` and `/export` (HTML or JSONL).
- `SessionManager` is "authoritative for finalized model context"; setting `session.agent.state.messages` does not replace it.

**INFERENCE:** Your channel/thread model can map one Pi session file to one task thread, and `parentSession` gives a ready link from a task thread back to its parent. For a central database you must wrap or replace `SessionManager` or sync the JSONL files. I did not verify a pluggable storage interface for `SessionManager`; check its exported declarations before you commit to a database design.

## 7. Sub-agents and multi-agent

**FACT:**

- Pi core: no built-in sub-agents. `PI/README.md`: "Pi ships with powerful defaults but skips features like sub-agents and plan mode."
- Pattern: `PI/examples/extensions/subagent/` ("Delegate tasks to specialized subagents with isolated context windows"). Each subagent is a separate `pi` process. Agents are markdown files (`scout.md`, `planner.md`, `reviewer.md`, `worker.md`). It supports parallel and chained runs, streamed progress, usage tracking, and abort that kills child processes.
- Other examples: `plan-mode`, `handoff.ts`, `todo.ts`, `event-bus.ts`.
- omp has built-in sub-agents (section 9).

**INFERENCE:** Your "Task" concept (own thread, reports back, runs on a chosen device) maps better to "one Pi session per task, on the chosen device" than to Pi's example, because you need your own scheduler, thread store, and result reporting.

## 8. Remote execution

**FACT:**

- Pluggable operations for tools. `PI/dist/core/tools/bash.d.ts`: `BashOperations.exec(command, cwd, { onData, signal, timeout, env }) => Promise<{ exitCode }>`, documented as "Override these to delegate command execution to remote systems (for example SSH)". Similar `ReadOperations`, `WriteOperations`, `EditOperations`. `createBashTool({ operations })`, `createReadTool`, `createEditTool`, `createWriteTool`, `createPowerShellTool` are exported from the SDK (`sdk.d.ts`).
- `PI/examples/extensions/ssh.ts`: "When --ssh is provided, read/write/edit/bash run on the remote" (via spawned `ssh` commands; needs key auth and bash on the remote). Also `examples/extensions/sandbox`, `gondolin` (VM sandbox), and `PI/docs/containerization.md`. The `user_bash` event can return `operations` to redirect user shell commands.
- RPC mode is stdio JSONL (`rpc.md`). There is no network server, no auth layer, and no multi-client protocol in Pi itself. Windows support is documented (`PI/docs/windows.md`).
- `PI/docs/security.md`: Pi has no sandbox. "Pi's tools and extensions run with the permissions of the Pi process."

**INFERENCE:** Two workable architectures. (A) One orchestrator process runs Pi sessions and swaps tool `Operations` to SSH or to your own device agent. All model calls and credentials stay central. Simple, but needs SSH or an agent on each device, and a Windows box needs a PowerShell path. (B) A small Pi-based "device runner" on each device, spoken to over RPC (stdio over SSH, or your own socket wrapper). The model call, tools, and MCP servers are all local to the device. This matches "tasks run on the user's own devices" and "LAN MCP servers" better, but you must distribute provider credentials per device (see the OAuth refresh note in section 3) or run a credential broker. Pi gives you the seams (Operations, RPC) but no ready remote runner.

## 9. Oh My Pi

**FACT, what it is:** `omp` is a hard fork of Pi, not an extension pack and not a wrapper. README (`gh api repos/can1357/oh-my-pi/readme`): "Built by Stencil Labs . Fork of Pi by @mariozechner", tagline "A coding agent with the IDE wired in", "60+ providers, 31 built-in tools, 14 lsp ops, 28 dap ops, ~80k lines of Rust core". The local binary at `OMP_BIN` is a 217 MB arm64 Mach-O executable (Bun-compiled). Strings in it point to `github.com/can1357/oh-my-pi` and packages `@oh-my-pi/pi-natives` and `@oh-my-pi/pi-catalog`. It is managed by "Tern" (`~/Library/Application Support/Tern/omp/manager.json`: `{"manager":"tern","name":"Tern"}`; `state.json` shows `installed.version 18.13.3`, link `~/.local/bin/omp`).

**FACT, divergence from Pi:** `docs/porting-from-pi-mono.md` records the "Last Sync Point" with upstream Pi as 2026-03-22. So omp does not track Pi 1.x. Its monorepo has its own `packages/ai`, `agent`, `coding-agent`, `tui`, `natives` (Rust), `catalog`, `collab-web`, `stats`, `wire`, `mnemopi`, and more. Package name `@oh-my-pi/pi-coding-agent` 18.13.3, MIT.

**FACT, what it adds (from `omp --help`, `docs/`):**

- Tools: `read, bash, edit, write, grep, glob, lsp, python, notebook, browser, computer` (desktop capture and input, disabled by default), `task` (sub-agents), `todo`, `web_search`, `ask`. Docs also list `ast-edit`, `ast-grep`, `debug` (DAP), `github`, `memory_edit`, `recall`, `reflect`, `retain`, `checkpoint`, `rewind`, `generate_image`, `tts`.
- Sub-agents built in: `task` tool with batch fan-out, bundled agents (`omp agents unpack`), isolation (worktree and a native isolation backend, `crates/pi-iso`), background jobs, agent registry with idle parking and revival, `omp cleanse` (parallel subagents), `--restore` (resume a crashed session "subagents included"). Source: `docs/tools/task.md`.
- Providers: env var list in `--help` covers Anthropic (API key and OAuth), OpenAI, Gemini, Copilot, Azure, Groq, Cerebras, xAI, OpenRouter, Mistral, z.ai, MiniMax, Cursor, Cline, Bedrock, Vertex, and more. `docs/providers.md`: `openai-codex` OAuth (ChatGPT) with `OPENAI_CODEX_OAUTH_TOKEN`; `fireworks` with `FIREWORKS_API_KEY`; multi-account OAuth ranking and rotation per org or workspace.
- Auth tooling: `omp login`, `omp auth` (export or import credentials), `omp auth-broker` (credential vault), `omp auth-gateway` (HTTP forward proxy backed by the broker, or JSON lines on stdio), `omp token`, `omp usage`.
- Remote and multi-machine: `omp ssh` (manage SSH host configs: add, remove, list; flags `--host --user --port --key`), `omp session pack|unpack|moved` ("Hand a session to another machine"), `omp acp` (Agent Client Protocol server over stdio), `omp collab` and `omp join` (shared sessions), `--mode json|rpc|rpc-ui`, `--profile` (isolated auth, sessions, settings), `omp worktree`.
- Other: LSP (14 ops) and DAP, TTSR stream rules, advisor runtime, "prewalk" and plan-yolo model switching, `--max-cost` and `--warn-cost`, approval modes, `--from-claude` and `--from-codex` session import, memory backends, voice tools.
- Embedding: SDK `createAgentSession` from the package root (`docs/sdk.md`), with `session.steer(text, images?, {attribution?})`, `followUp(...)`, `abort()`, `prompt(..., {streamingBehavior})`, `customTools`. RPC doc says host apps can answer "host-tool" and "host-URI" requests, so a host can supply tools over RPC (`docs/rpc.md`: stdin carries "host-tool updates/results"). I did not read the host-tool section in detail.
- No Claude Agent SDK provider found in `docs/providers.md` (my grep for "agent sdk" and "claude-agent" found nothing). `--from-claude` only imports sessions.

**FACT, maintenance:** MIT. 37,001 stars, 4,260 open issues, created 2025-12-31, last push 2026-10-29. Releases: v18.13.3 (2026-10-28), 18.13.2 (10-27), 18.13.1 (10-27), 18.13.0 (10-26), 18.12.6 (10-21), 18.12.5 (10-21), 18.12.4 (10-20), 18.12.3 (10-19), 18.12.2 (10-18), 18.12.1 (10-17), 18.12.0 (10-17), 18.11.3 (10-16). That is 12 releases in 13 days. Top contributors by commits: `can1357` 13,235, `roboomp` 4,333 (a bot account), `H4vC` 2,340, `will-bogusz` 1,690, `badlogic` 1,343 (Pi history), then smaller counts. The binary's embedded changelog cites PR numbers up to about #16978. Requires Bun `>=1.4.4` (`packages/coding-agent/package.json` `engines.bun`); the package `exports` point at raw `.ts` sources (`./src/index.ts`), so it is Bun-only with a Rust native addon.

**INFERENCE:** omp is very active, feature-rich, and closer to your product on sub-agents, SSH hosts, credential broker, and session hand-off. It is also a large, fast-moving, Bun-only, partly bot-authored codebase with a single main maintainer (13k of its commits). Embedding it means tracking a surface that changes daily.

## 10. Risks

**FACT:**

- **API churn (Pi):** 0.79.x in June 2026, 0.99.0 on 2026-09-29, 1.0.0 on 2026-10-01, 1.4.0 on 2026-10-21. About one minor release per week after 1.0. `PI/CHANGELOG.md` is 6,263 lines and has 49 "Breaking Changes" headings. Examples inside one changelog: default TUI mode changed in 1.0.0; provider id `azure-openai-responses` renamed to `azure`; pi-ai moved from a global API to `createModels()`/`Models` ("Migrating from the Old Global API" in the pi-ai README); the `@earendil-works/pi-ai/oauth` entry now holds types only and built-in login flows are "private provider implementations".
- **Ownership move:** repo and npm scope changed (`badlogic/pi-mono` to `earendil-works/pi`; the docs say Radius is "crafted for Pi by the builders of Pi, Earendil Works").
- **Bus factor (Pi):** 4,030 commits by `badlogic`, 919 by `mitsuhiko`, then 365 and below. External PRs and issues are auto-closed unless a maintainer vouches.
- **Runtime:** Pi needs Node `>=22.19.0`; works in Bun too (`sdk.md`: "Node.js or Bun process"; a `build:binary` script uses `bun build --compile`). omp needs Bun `>=1.4.4`.
- **Dependency pinning:** issue `earendil-works/pi#7564` reports that exact pins on SDK dependencies in pi-ai force duplicate copies in consumers (state: closed). The installer "pins all dependencies"; plain npm "does not pin transitive dependencies" (`PI/README.md`).
- **No sandbox:** tools run with the process user's permissions (`security.md`).
- **OAuth coupling:** ChatGPT login uses a fixed loopback port 1455 and Codex-like flows (`openai-chatgpt.js`). Anthropic subscription use by third parties is restricted by Anthropic's terms (section 4). I did not verify OpenAI's terms for third-party use of ChatGPT sign-in.

**INFERENCE:**

- Pin exact versions and wrap Pi behind your own `AgentRuntime` interface. Expect a migration every few weeks if you track latest.
- Token sharing across several devices is a design problem: refresh tokens rotate, and Pi's lock lives in one credential store. A broker (like omp's `auth-broker`) or a device-local login avoids double-refresh.
- Pi's SDK is documented as in-process and single-user (file-backed settings, `~/.pi/agent`). For a multi-tenant or multi-channel app, give each runtime an explicit `agentDir`, `cwd`, `SessionManager`, `SettingsManager`, and `ResourceLoader` so nothing reads the global dir.

---

## Recommendation

**Pick Pi (the `@earendil-works/*` packages), not Oh My Pi, as the embedded harness. Do not make Pi your only harness.**

Why Pi:

1. It meets two of your three day-one providers natively: OpenAI via Sign in with ChatGPT (plus legacy Codex OAuth) and Fireworks via API key. Anthropic API key also works (FACT, sections 3).
2. It has a documented, typed SDK and a documented RPC mode, with MIT license and Node or Bun support (sections 1, 2).
3. It has the seams you need for remote execution: tool `Operations` (bash, read, write, edit), a `createPowerShellTool`, an SSH example, and RPC over stdio (section 8). MCP, skills, compaction, session tree, and cross-provider model switching are built in (sections 3, 5, 6).
4. Its published package ships `.d.ts` files and docs; omp ships raw TypeScript for Bun and a Rust addon.

Why not omp as the dependency: Bun-only, daily releases, huge surface, bot-heavy history, and a fork that stopped syncing with Pi in March 2026. It has features you want (task sub-agents, `omp ssh`, session pack/unpack, `auth-broker`, host tools over RPC). **INFERENCE:** read its docs for design ideas, and consider it only if you accept a Bun-only runtime and pin to one release.

Why not "Pi only": the Claude Agent SDK has no official Pi integration (section 4). Use a runtime interface with at least two adapters:

- `PiRuntime` for OpenAI Codex/ChatGPT, Fireworks, and Anthropic API key.
- `ClaudeAgentSdkRuntime` that runs the SDK natively on the user's device with the user's own Claude Code sign-in, or an API key. The community `claude-agent-sdk-pi` provider is a fallback if you must keep one loop in Pi, with the weak spots listed in section 4.

Main integration risks, in order:

1. **Anthropic path.** No official Pi support; the subscription rules forbid routing plan credentials through a third-party product. Decide early: API key or user-side Claude Code sign-in.
2. **Multi-device credentials.** Rotating OAuth refresh tokens and a fixed loopback port 1455. Need a broker or per-device login.
3. **Remote execution is a pattern, not a product.** You build the device runner, SSH or RPC transport, Windows shell handling, and permission model. Pi has no sandbox.
4. **API churn.** Weekly releases, 49 breaking-change notes, repo and scope move. Pin exact versions and add an adapter layer.
5. **Single-user assumptions.** Global `~/.pi/agent` state. Always pass explicit `agentDir`, `cwd`, and managers.
6. **Session store.** JSONL files, not a database. Check `SessionManager` for a storage seam before you design channel history around it.
7. **Bus factor and contribution gate.** Two main maintainers; outside PRs are auto-closed. Plan to carry patches as extensions, not forks.

Suggested first spike (about one day): `createAgentSession` with `SessionManager.inMemory()`, an explicit `agentDir`, a custom `BashOperations` that runs over SSH to one Linux host, `prompt` with `streamingBehavior: "steer"`, a model switch from `fireworks` to `openai` mid-session, and the MCP extension pointed at one LAN server. This tests sections 2, 3, 5, 8 in one pass.

## Sources

Local:
- `PI/package.json`, `PI/README.md`, `PI/CHANGELOG.md`
- `PI/docs/{sdk,rpc,rpc-commands,cli-integration,providers,models,mcp,extensions,skills,sessions,session-format,custom-provider,security,configuration,compaction}.md`
- `PI/dist/core/sdk.d.ts`, `PI/dist/core/agent-session.d.ts`, `PI/dist/core/tools/bash.d.ts`
- `PI/examples/sdk/*.ts`, `PI/examples/extensions/{ssh.ts,subagent/,custom-provider-anthropic/}`
- `PI_DEPS/pi-ai/README.md`, `PI_DEPS/pi-ai/dist/providers/fireworks.js`, `PI_DEPS/pi-ai/dist/auth/oauth/{openai-chatgpt,openai-codex}.js`
- `PI_DEPS/pi-agent-core/README.md`
- `omp --help`, `omp ssh --help`, `omp session --help`, `omp login --help`
- `~/Library/Application Support/Tern/omp/{manager.json,state.json}` (no credentials)

Remote:
- https://github.com/earendil-works/pi (via `gh api`: repo, releases, contributors, issues 9992, 3808, 7564)
- https://github.com/can1357/oh-my-pi (README, `docs/sdk.md`, `docs/rpc.md`, `docs/providers.md`, `docs/tools/task.md`, `docs/porting-from-pi-mono.md`, `packages/coding-agent/package.json`, releases, contributors)
- https://github.com/prateekmedia/claude-agent-sdk-pi (README)
- https://code.claude.com/docs/en/legal-and-compliance (Anthropic terms, fetched 2026-10-28)
- https://gigazine.net/gsc_news/en/20260220-anthropic-third-party-block (secondary, about subscription enforcement)
