# Provider and auth research: Anthropic, OpenAI (ChatGPT login), Fireworks

Research date: 2026-10-28. Method: read official docs, SDK registries, and public source files. No logins, no local credential files read.

Labels used in this report:

- **FACT**: a primary source states it, or a public source file shows it. The URL follows.
- **INFERENCE**: my reasoning from facts. Treat as a hypothesis.
- **SECONDARY**: a news article or blog, not a primary source. Confirm before you rely on it.
- **UNVERIFIED**: I did not confirm it in this pass.

Tool-call budget note: this pass used about 59 calls, above the 50 target. Gaps are listed in "Open gaps" at the end.

---

## Headline findings

1. **OpenAI now has an official path for open-source and locally hosted apps to use a ChatGPT plan** ("Sign in with ChatGPT", plan usage, announced at DevDay 2026-09-29, SECONDARY for the date). It uses dynamic client registration, OAuth PKCE, and `POST https://api.openai.com/v1/responses` with a bearer token. This replaces the old pattern of copying the Codex CLI client id and calling `chatgpt.com/backend-api/codex`. (https://developers.openai.com/siwc/token-sharing-open-source)
2. **Anthropic does not allow third-party developers to offer claude.ai login, or to collect or store claude.ai tokens.** The one explicit carve-out is an end user signing in to the unmodified Claude Code binary with their own plan. (https://code.claude.com/docs/en/legal-and-compliance)
3. **The Agent SDK ships the Claude Code binary as per-platform npm packages**, including `win32-x64`, so no separate install is needed on Mac, Windows, or Linux. (https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk)
4. **Pi (now `earendil-works/pi`, v1.4.0) already has providers for all three**: Anthropic (API key and subscription OAuth), OpenAI (Sign in with ChatGPT, plus a "legacy" Codex provider), and Fireworks (OpenAI and Anthropic compatible). (https://github.com/earendil-works/pi/blob/main/packages/ai/README.md)

---

## A. Claude Agent SDK (TypeScript)

### A1. Version and core API

- **FACT** Latest version is `0.3.312`, published 2026-10-27. Node `>=18.14.0`. Peer dependencies: `zod ^4`, `@anthropic-ai/sdk >=0.93.0`, `@modelcontextprotocol/sdk ^1.29.0`. License field: "SEE LICENSE IN README.md". The SDK is governed by Anthropic's Commercial Terms. (https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk, https://code.claude.com/docs/en/agent-sdk/overview)
- **FACT** Changelog: https://github.com/anthropics/claude-agent-sdk-typescript/blob/main/CHANGELOG.md. The SDK moves fast (daily patch releases in late October 2026). Pin versions.

`query({ prompt, options })` returns a `Query` that extends `AsyncGenerator<SDKMessage>`. **FACT** for everything in this list: https://code.claude.com/docs/en/agent-sdk/typescript

| Need | Option or API |
|---|---|
| Streaming input | `prompt` can be `AsyncIterable<SDKUserMessage>`. `Query.streamInput(stream)` adds messages later. `interrupt()` works in streaming input mode. |
| Resume and fork | `resume`, `continue`, `forkSession`, `resumeSessionAt`, `sessionId`, `persistSession`. Functions: `listSessions`, `getSessionInfo`, `getSessionMessages`, `renameSession`, `tagSession`, `forkSession(id, { upToMessageId, title })`. `sessionStore` (alpha) mirrors transcripts to external storage. |
| Custom tools (in process) | `tool(name, description, zodShape, handler, extras)` and `createSdkMcpServer({ name, tools })`, passed in `mcpServers`. Tool names become `mcp__<server>__<tool>`. (https://code.claude.com/docs/en/agent-sdk/custom-tools) |
| Hooks | `hooks` keyed by `HookEvent`. Events include `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `UserPromptSubmit`, `SessionStart`, `SessionEnd`, `Stop`, `SubagentStart`, `SubagentStop`, `PreCompact`, `PermissionRequest`, `TaskCreated`, `TaskCompleted`, `Elicitation`, `WorktreeCreate`, and more. |
| Permission callback | `canUseTool(toolName, input, { signal, suggestions, toolUseID, requestId }) => Promise<PermissionResult \| null>`. It runs only when permission evaluation falls through to a prompt. Returning `null` blocks the call indefinitely. `permissionMode` values: `default`, `plan`, `bypassPermissions`, `auto`, others. |
| Subagents | `agents: Record<string, AgentDefinition>`. `agent` picks the main-thread agent. `forwardSubagentText` and `agentProgressSummaries` stream subagent output. (`AgentDefinition` fields not read in this pass.) |
| Settings | `settingSources: ('user'\|'project'\|'local')[]`. `[]` disables filesystem settings. `settings` and `managedSettings` give inline settings. |
| System prompt | A string, or `{ type: 'preset', preset: 'claude_code', append?, excludeDynamicSections? }`. |
| Paths | `cwd`, `additionalDirectories` (passed as `--add-dir`). |
| Cancel | `abortController`, `Query.interrupt()`, `Query.close()`, `stopTask(id)`. |
| Model | `model`, `fallbackModel`, `Query.setModel()`, `effort` (`low` to `max`), `thinking` (adaptive by default), `supportedModels()`. |
| Limits | `maxTurns`, `maxBudgetUsd` (client-side estimate, can overshoot), `taskBudget` (alpha). |
| Cost and usage | `SDKResultMessage` (success arm) has `total_cost_usd`, `usage`, `modelUsage` (per model), `num_turns`, `duration_ms`. Error subtypes: `error_max_turns`, `error_during_execution`, `error_max_budget_usd`, `error_max_structured_output_retries`. Docs call `total_cost_usd` "an estimate, not a billing statement". |
| Warm start | `startup()` pre-spawns the subprocess. `prewarm()` (alpha) holds a spare process, about 230 to 260 MB each. |

### A2. Process model and runtime

- **FACT** The SDK is "a library that runs the Claude Code binary". (https://code.claude.com/docs/en/agent-sdk/overview) Each `query()` talks to a Claude Code subprocess.
- **FACT** Per-platform optional dependencies exist at the same version: `linux-x64`, `linux-arm64`, `linux-x64-musl`, `linux-arm64-musl`, `darwin-x64`, `darwin-arm64`, `win32-x64`, `win32-arm64`. (https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk) `pathToClaudeCodeExecutable` is "needed only if optional dependencies were skipped". `executable` picks the JS runtime (`node`, `bun`, `deno`). `spawnClaudeCodeProcess` lets you spawn on a VM, container, or remote host. (https://code.claude.com/docs/en/agent-sdk/typescript)
- **FACT** The changelog has Windows-specific fixes (0.3.261 plugin delivery over stdin for Windows start failures, 0.3.271 mapped-drive session lookup, 0.3.309 embedded CLI extraction to a per-user temp directory on Windows). (https://github.com/anthropics/claude-agent-sdk-typescript/blob/main/CHANGELOG.md)
- **INFERENCE** The SDK runs on Windows. Evidence is the `win32-x64` package and the Windows fixes. I did not find a docs page that states "supported on Windows" and did not test it.
- **INFERENCE** Node 18.14 or newer, or Bun, on each device is enough. No separate Claude Code install is needed when the optional dependency installs. Each running task costs one subprocess, so task fan-out equals process fan-out.
- **FACT** For non-JS hosts, the CLI can run as a subprocess with `-p --output-format json`. (https://code.claude.com/docs/en/agent-sdk/overview)

### A3. Authentication and policy

Supported auth, **FACT**:

- API key: `ANTHROPIC_API_KEY` (sent as `X-Api-Key`).
- Gateway bearer: `ANTHROPIC_AUTH_TOKEN`.
- Cloud: `CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_USE_VERTEX`, `CLAUDE_CODE_USE_FOUNDRY`.
- Subscription login from `/login` (Pro, Max, Team, Enterprise).
- Long-lived token: `claude setup-token` prints a one-year OAuth token. Set it as `CLAUDE_CODE_OAUTH_TOKEN`. It "authenticates with your Claude subscription", needs Pro, Max, Team, or Enterprise, and "can only make model requests".
- Precedence: cloud provider, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_API_KEY`, `apiKeyHelper`, `CLAUDE_CODE_OAUTH_TOKEN`, profiles, then `/login` credentials.
- Credential storage: macOS Keychain (fallback `~/.claude/.credentials.json`), Linux `~/.claude/.credentials.json`, Windows `%USERPROFILE%\.claude\.credentials.json`.
- Source for all of the above: https://code.claude.com/docs/en/authentication

**Current policy text (FACT, fetched 2026-10-28).** From https://code.claude.com/docs/en/legal-and-compliance, section "Authentication and credential use":

> "**OAuth authentication** is intended exclusively for purchasers of Claude Free, Pro, Max, Team, and Enterprise subscription plans and is designed to support ordinary use of Claude Code and other native Anthropic applications."

> "**Developers** building products or services that interact with Claude's capabilities, including those using the Agent SDK, should use API key authentication through Claude Console or a supported cloud provider. Anthropic does not permit third-party developers to offer Claude.ai login into their own applications, or to route requests through Free, Pro, or Max plan credentials on behalf of their users. Moreover, developers may not collect, store, or intermediate Claude.ai credentials or session tokens — sign-in to a Claude account must complete through Anthropic's own flow."

> "Nor does it prevent an end user from signing in to the unmodified Claude Code binary with their own Claude subscription, including where a platform hosts Claude Code as described under *Can customers offer Claude Code in their products?* above."

> "Anthropic reserves the right to take measures to enforce these restrictions and may do so without prior notice."

Same page, usage policy: "Advertised usage limits for Pro and Max plans assume ordinary, individual usage of Claude Code and the Agent SDK."

Agent SDK overview note (FACT, https://code.claude.com/docs/en/agent-sdk/overview):

> "Unless previously approved, Anthropic does not allow third party developers to offer claude.ai login or rate limits for their products, including agents built on the Claude Agent SDK. Use the API key authentication methods described in the Quickstart instead."

Conditions for running Claude Code inside a product (FACT, same legal page): the binary must not be modified, and customers must not remove or restrict any built-in authentication method. Customers may not "pay for, resell, or intermediate Claude usage on their end users' behalf". Each end user must authenticate with their own API key, plan credentials, or cloud credentials.

**Timeline of enforcement and change:**

- **SECONDARY** Jan 2026: Anthropic engineer Thariq Shihipar said Anthropic had taken steps against tools "spoofing the Claude Code harness" and wrote "Third-party harnesses using Claude subscriptions create problems for users and are prohibited by our Terms of Service". (https://www.theregister.com/software/2026/02/20/anthropic-clarifies-ban-on-third-party-tool-access-to-claude/5014546)
- **SECONDARY** Feb 19 to 20, 2026: the legal page said "Using OAuth tokens obtained through Claude Free, Pro, or Max accounts in any other product, tool, or service — including the Agent SDK — is not permitted". OpenCode removed Claude Pro/Max login the same week, citing "anthropic legal requests". (same Register URL)
- **FACT (negative)** That exact sentence is not on the legal page today. One blog says it was later removed (https://autonomee.ai/blog/claude-code-terms-of-service-explained/, SECONDARY). The current wording is the quote above.
- **SECONDARY** April 2026 enforcement against OpenClaw, OpenCode, Cursor and others, and a May 2026 partial reversal, come from a news summary (https://medium.com/@SyedAbbasT/anthropic-cuts-off-openclaw-and-third-party-tools-from-claude-subscriptions-a15cf1518fda, https://aitoolsreview.co.uk/insights/anthropic-blocks-openclaw-policy-shift). The sources conflict. I did not find a primary source. UNVERIFIED.
- **FACT** Support article "Use the Claude Agent SDK with your Claude plan" (https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan): June 15, 2026 notice: "We've paused the previously-announced changes to Claude Agent SDK usage." It says nothing has changed for now and that Agent SDK, `claude -p`, and "third-party app usage still draw from your subscription limits". Oct 7, 2026 notice: Max and Team plans now include monthly API credits that cover the Claude API, Managed Agents, the Agent SDK and `claude -p`, but only "with an API key from your linked Claude Console organization".
- **INFERENCE** The support page (usage draws from subscription limits) and the legal page (developers must not offer plan login) do not contradict each other if you read them this way: the end user may sign in themselves, and the developer may not build the login or hold the token.

**Personal use versus distributing an app (INFERENCE, not legal advice):**

| Case | What looks allowed | What looks forbidden |
|---|---|---|
| Personal use, you on your own machines | You sign in to the bundled, unmodified Claude Code with your own plan (`claude` login, or `claude setup-token` for yourself). The app only spawns the SDK. The SDK doc says plan limits assume "ordinary, individual usage of ... the Agent SDK". | Pasting your token into an app that stores it, and running 24/7 automated fleets, may exceed "ordinary, individual usage". Enforcement is "without prior notice". |
| Distributing an open-source app | Default to API key, Bedrock, Vertex, or Foundry. Optionally say "if you already ran `claude` login on this machine, the SDK will use it" and never touch the token. | A "Log in with Claude" button, an OAuth flow in your app, a token field that stores `CLAUDE_CODE_OAUTH_TOKEN`, a hosted relay, or pooling users' plans. All match "offer Claude.ai login", "collect, store, or intermediate", or "route requests through ... plan credentials". |

### A4. Custom tools only, and custom base URL

- **FACT** `tools: []` removes all built-in tools: "Claude can only use your MCP tools." `tools: ['Read','Grep']` keeps only those built-ins. A bare name in `disallowedTools` removes a tool from context. `allowedTools` only pre-approves and does not restrict. `toolAliases` maps built-in names to your MCP tools (for example `{ Bash: 'mcp__workspace__bash' }`). `strictMcpConfig` ignores project and user MCP config. (https://code.claude.com/docs/en/agent-sdk/custom-tools, https://code.claude.com/docs/en/agent-sdk/typescript)
- **FACT** SDK MCP tools are deferred by default (tool search). Use `alwaysLoad: true` to keep schemas in the first prompt.
- **INFERENCE** For a clean "our tools only" engine use: `tools: []`, `settingSources: []`, `strictMcpConfig: true`, a custom `systemPrompt` string, `persistSession` as needed, and `permissionPrompts`/`canUseTool` routed to your UI. Test that Claude Code still injects its own system text.
- **FACT** `ANTHROPIC_BASE_URL` is supported for LLM gateways that expose a supported API format. But: "Anthropic doesn't endorse, maintain, or audit third-party gateway products, and doesn't support routing Claude Code to non-Claude models through any gateway." (https://code.claude.com/docs/en/llm-gateway)
- **FACT** Setting only `ANTHROPIC_BASE_URL` without a gateway credential does not replace a subscription login. The saved claude.ai login stays active. Set `ANTHROPIC_AUTH_TOKEN` or `ANTHROPIC_API_KEY` to the gateway credential. (same URL)
- **FACT** Fireworks documents its own Claude Code setup using `ANTHROPIC_BASE_URL=https://api.fireworks.ai/inference` with `ANTHROPIC_AUTH_TOKEN`. (https://docs.fireworks.ai/tools-sdks/anthropic-compatibility.md, https://docs.fireworks.ai/ecosystem/integrations/claude-code)
- **INFERENCE** Verdict: Anthropic-side it is a **hack, not supported**. The vendor (Fireworks) supports it on their side. Known gaps on Fireworks: no server tools (web search, web fetch, code execution), `count_tokens` returns 404, no tool-schema `cache_control`. Expect model-name mapping work (`ANTHROPIC_MODEL` style variables) and odd failures when the SDK assumes Claude features.

### A5. Coexistence with Pi (all INFERENCE unless marked)

- **FACT** Pi's provider unit "owns its model catalog, its auth ... and its stream behavior". (https://github.com/earendil-works/pi/blob/main/packages/ai/README.md) That is an LLM-call abstraction, not an agent abstraction. The Agent SDK is a full agent loop with its own tools, sessions, and permissions.
- **Option 1 (recommended): separate engine per task.** One task equals one `query()` subprocess. Adapt `SDKMessage` events to the app's own event schema. Give it your tools through `createSdkMcpServer`. Use `canUseTool` and hooks for approvals. Pi runs other tasks in its own loop. The app owns the common task, session, and event types.
- **Option 2: Anthropic inside Pi as a plain provider.** Use Pi's native Anthropic provider with an API key. This is a Messages API call. You lose Claude Code's built-in tools, skills, compaction, and subagents, but you keep one loop and one tool system.
- **Option 3 (avoid): wrap the Agent SDK as a Pi "provider".** It nests two agent loops, duplicates tool execution, and hides tool calls from Pi.
- **FACT** Pi can be driven four ways: interactive, print or JSON mode, RPC, and a TypeScript SDK. It requires Node 22.19 or newer. It "has no built-in permission system". (https://github.com/earendil-works/pi) So the app must add sandboxing around Pi tasks.

### A policy risks

1. **Plan login in a distributed app is forbidden** (quotes in A3). Highest risk. Do not ship an Anthropic OAuth flow, token field, or relay.
2. **Pi ships Anthropic subscription OAuth.** Pi's README lists "Anthropic (Claude Pro/Max subscription)" as an OAuth provider and notes that "Anthropic OAuth" keeps a fixed client identity. (https://github.com/earendil-works/pi/blob/main/packages/ai/README.md) Using that path in your product would sit on the forbidden side of the policy above. INFERENCE: do not enable it in the app, even though Pi supports it.
3. **Policy has changed several times in 2026 and enforcement is "without prior notice".** Re-read the legal page before each release.
4. **Branding.** "Claude Code" and "Claude Code Agent" are not permitted product names. Allowed: "Claude Agent", "{YourAgentName} Powered by Claude". Anthropic's own "Claude Duet" name is not yours to use for a clone either (INFERENCE). (https://code.claude.com/docs/en/agent-sdk/overview)
5. **Unsupported non-Claude routing** through `ANTHROPIC_BASE_URL` (A4).
6. **Binary must stay unmodified** and built-in auth methods must not be removed or restricted. (legal page)

---

## B. OpenAI through ChatGPT login

### B1. How Codex CLI "Sign in with ChatGPT" works (FACT from public source and docs)

From `openai/codex` source, `codex-rs/login/src/server.rs` and `auth/manager.rs` (https://github.com/openai/codex/blob/main/codex-rs/login/src/server.rs, https://github.com/openai/codex/blob/main/codex-rs/login/src/auth/manager.rs):

- Issuer: `https://auth.openai.com` (`DEFAULT_ISSUER`).
- Local callback server: default port `1455`. Redirect URI: `http://127.0.0.1:{port}/auth/callback`.
- Flow: OAuth authorization code with PKCE (`pkce.rs`), plus an `originator` parameter (default value `codex_cli_rs`).
- Scope sent by the CLI: `openid profile email offline_access api.connectors.read api.connectors.invoke`.
- Refresh: `https://auth.openai.com/oauth/token`. `TOKEN_REFRESH_INTERVAL` is 8 (days). Env overrides exist (`CODEX_REFRESH_TOKEN_URL_OVERRIDE`, `CODEX_APP_SERVER_LOGIN_CLIENT_ID`).
- Device code flow: exists. Endpoints `{auth_base}/deviceauth/usercode` and `{auth_base}/deviceauth/token` (`device_code_auth.rs`). Command: `codex login --device-auth`. Docs call it beta and the preferred method for remote or headless setups. It must be enabled in ChatGPT security settings (personal) or workspace permissions (admin). (https://learn.chatgpt.com/docs/auth)
- Token storage: controlled by `cli_auth_credentials_store`: `keyring`, `file` (`$CODEX_HOME/auth.json`, default `~/.codex/auth.json`), `auto`, `ephemeral`. Docs: "treat `~/.codex/auth.json` like a password: it contains access tokens." "Codex refreshes tokens automatically during use before they expire." Headless fallback: copy `auth.json` from a machine with a browser, or forward `localhost:1455`. (https://learn.chatgpt.com/docs/auth)
- **UNVERIFIED** The OAuth `client_id` string of Codex CLI. I did not find the constant in the files I read. Community tools reuse it, but you should not rely on it (see B3).

### B2. Endpoints, headers, models, limits

- **FACT (legacy ChatGPT-backend path)** Codex source defines `CHATGPT_CODEX_BASE_URL = "https://chatgpt.com/backend-api/codex"`. (https://github.com/openai/codex/blob/main/codex-rs/model-provider-info/src/lib.rs) INFERENCE: requests go to `.../responses` under that base.
- **FACT** Header names seen in Codex source: `originator` (`codex_cli_rs`), `User-Agent`, a ChatGPT account id header (`CHATGPT_ACCOUNT_ID_HEADER`), `OpenAI-Beta`, `x-codex-installation-id`, `x-codex-turn-state` (sticky routing, must be replayed within a turn), `x-codex-turn-metadata`, `x-codex-parent-thread-id`, `x-codex-window-id`. (https://github.com/openai/codex/blob/main/codex-rs/core/src/client.rs, https://github.com/openai/codex/blob/main/codex-rs/login/src/auth/default_client.rs) The literal value of the account header is UNVERIFIED.
- **FACT (new official path)** `POST https://api.openai.com/v1/responses`, `Authorization: Bearer <ACCESS_TOKEN>`, body must have `store: false` and `stream: true`. The doc says to use "the public Responses API endpoint", not "ChatGPT's `backend-api` endpoints". List models with `GET https://api.openai.com/v1/models` using the same token, keep entries where `visibility` is `"list"`, use `slug` as the model id. "Treat [model/list] as a catalog, not an entitlement check." (https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference, https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server)
- **FACT** Responses limits for this route (https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations): system role messages are rejected (use `instructions` or developer messages). Omit `background`, `conversation`, `max_output_tokens`, `max_tool_calls`, `metadata`, `moderation`, sampling settings, `user`, and `previous_response_id` over HTTP (send full history in `input`). Function and custom tools work (group in namespaces or use `additional_tools`). Not supported: image generation, file search, code interpreter, native computer use, hosted MCP or connectors, `tool_search`, audio and video input, Files API, transcription API.
- **FACT** Usage-limit errors can arrive as `response.failed` after streaming starts, with codes such as `subscription_sharing_usage_limit_exceeded` and `subscription_sharing_usage_unavailable`. Treat only `response.completed` as success. (models-and-inference URL above)
- **FACT** Plans: "Eligible ChatGPT Plus and Pro users can use their ChatGPT plan for AI requests in participating apps." Business, Enterprise, and Free are not mentioned. (https://developers.openai.com/siwc/quickstart.md) Users "manage app usage and access in ChatGPT settings". Limits are per plan and not numeric on the pages I read. A single registration "shares plan usage settings and limits" across that user's hosts for the same workspace. (open-source overview URL below)
- **SECONDARY** A guide says users set a weekly share cap per app, and that the access token lasts 1 hour and the refresh token 30 days. (https://dev.to/hassann/sign-in-with-chatgpt-for-developers-the-oauth-flow-plan-usage-and-what-it-means-for-your-api-bill-48f7, https://apidog.com/blog/sign-in-with-chatgpt/) The primary doc sample shows `"expires_in": 3600` only.
- **UNVERIFIED** The exact model names available (GPT-5.x Codex models per Pi's README, https://github.com/earendil-works/pi/blob/main/packages/ai/README.md). Always read the live `/v1/models` list.

### B3. OpenAI's stance on third-party use of ChatGPT plan login

**Official program (FACT).** "Sign in with ChatGPT" has a "ChatGPT plan usage" option:

> "ChatGPT plan usage is available to all open-source partners and selected private clients." (https://developers.openai.com/siwc/quickstart.md)

> Plan usage is "an optional capability within Sign in with ChatGPT" that lets an open-source app request permission to use the user's ChatGPT plan for eligible Responses API requests. "This does not grant access to their ChatGPT conversations or other account context." (https://developers.openai.com/siwc/token-sharing-open-source)

The commercial route is separate: "Sign in with ChatGPT is currently available to selected commercial partners through a limited trial." For paid or remotely hosted apps, the doc points to an interest form (https://openai.com/form/sign-in-with-chatgpt-interest/). The open-source docs say that program is for "open-source and locally hosted apps".

Open-source sign-in details (FACT, https://developers.openai.com/siwc/token-sharing-open-source/sign-in):

- Authorization endpoint `https://auth.openai.com/api/accounts/authorize`. Token endpoint `https://auth.openai.com/api/accounts/oauth/token`. OIDC discovery at `https://auth.openai.com/.well-known/openid-configuration`.
- First request uses `client_id=dynamic_agent_client` with `ext_agent_host_id` and `agent_name_hint`. The issued `oaiapp_...` id comes back in the callback. Do not save `dynamic_agent_client` as the issued id. Later sign-ins reuse the saved id.
- PKCE with `S256` is required. No client secret.
- Redirect: `http://127.0.0.1:1455/auth/callback`. "Do not substitute with `localhost`." Only the port may change.
- Scopes: `openid profile email` for identity. `offline_access resource.invoke chatgpt.tokens.use.direct` for plan usage, with resource `https://api.openai.com/v1`. "A valid ID token alone does not authorize ChatGPT plan usage. Check the granted scopes."
- Refresh: replace "the access token, expiry, granted scopes, and rotating refresh token together". The client "stays bound to its registered workspace". Store credentials with owner-only permissions (`0600`), never commit or log them.
- Device code: not covered on that page.

Self-hosted VMs (FACT, https://developers.openai.com/siwc/token-sharing-open-source/self-hosted-vms): create a stable host id per VM. Run OAuth locally (the `127.0.0.1` callback must reach the browser machine). Copy the registration's protected credential file to the VM over a secure channel such as SSH. The VM refreshes tokens later and keeps its own host id. Known gap, quoted: "Host-specific usage attribution and revocation of ChatGPT plan access for transferred sessions are not yet available." The page does not say whether one user may use the same tokens on several devices at once, or whether proxying tokens through a server is allowed. UNVERIFIED.

**Announcement (SECONDARY).** Sign in with ChatGPT started as a July 2026 beta for identity. At DevDay on 2026-09-29, OpenAI added plan usage with about 16 launch partners (reports name Cognition, Notion, Lovable, Vercel, OpenClaw, T3, Warp, Amp, Kilo Code). (https://thenewstack.io/sign-in-with-chatgpt/ headline only, https://securitybrief.asia/story/openai-launches-chatgpt-sign-in-for-16-partner-tools, https://dev.to/hassann/sign-in-with-chatgpt-for-developers-the-oauth-flow-plan-usage-and-what-it-means-for-your-api-bill-48f7) I could not read OpenAI's own DevDay recap (HTTP 403 at https://openai.com/index/devday-2026-recap/).

**Older third-party use of Codex CLI login (not official).**

- **SECONDARY** A blog says an OpenAI executive (Tibo Sottiaux) said ChatGPT accounts can be used inside third-party harnesses, and that Pi and OpenCode are about 10% of Codex traffic. I found no primary OpenAI policy page or quote. (search result, https://manifest.build/blog/chatgpt-plus-tokens-third-party-harnesses/)
- **SECONDARY** OpenCode added Codex login in v1.1.11 in Jan 2026. Whether that was a formal partnership is unclear. (https://blog.devgenius.io/jump-ship-in-minutes-codex-oauth-now-works-in-opencode-d2708c32f571) OpenCode's repository is now `anomalyco/opencode` (redirect from `sst/opencode`).
- **SECONDARY** Community plugins call it "personal use only" and put terms compliance on the user. (https://github.com/jxstanford/opencode-openai-codex-auth)
- **FACT** Pi now marks its Codex provider "legacy": "Superseded by Sign in with ChatGPT on the OpenAI provider." (https://github.com/earendil-works/pi/blob/main/packages/ai/README.md)
- **FACT** Codex docs only say: "Access tokens are intended for trusted scripts, schedulers, and private CI runners" and "Don't expose Codex execution in untrusted or public environments." (https://learn.chatgpt.com/docs/auth)
- **UNVERIFIED** OpenAI Terms of Use text on credential sharing and automated access. I did not read it. Read https://openai.com/policies/terms-of-use/ before launch.

### B4. Official alternatives for driving Codex as an engine

Versions (FACT): Codex CLI `rust-v0.165.0` released 2026-10-27 (https://api.github.com/repos/openai/codex/releases/latest). `@openai/codex-sdk` `0.165.0`, Node `>=18`, depends on `@openai/codex` 0.165.0 (https://registry.npmjs.org/@openai/codex-sdk).

| Surface | What it is | Custom tools | Source |
|---|---|---|---|
| `@openai/codex-sdk` | `new Codex()`, `startThread()`, `thread.run(prompt)`, `resumeThread(id)`. `config` and `configOverrides` set Codex config. "Use the library server-side". Docs do not describe the process model, `runStreamed`, or custom tool registration. | Not documented. INFERENCE: add tools through MCP servers in `config`. UNVERIFIED. | https://learn.chatgpt.com/docs/codex-sdk |
| `codex app-server` | JSON-RPC 2.0 over stdio (default), WebSocket (`--listen ws://IP:PORT`, "experimental and unsupported"), or Unix socket. Methods: `initialize` then `initialized`, `thread/start`, `thread/resume`, `thread/fork`, `turn/start`, `turn/steer`, `turn/interrupt`, `model/list`, MCP methods. Server requests to the client include `item/permissions/requestApproval` and `tool/requestUserInput`. `codex app-server generate-ts --out ./schemas` makes TypeScript types. | `dynamicTools` on `thread/start` is experimental and needs `capabilities.experimentalApi`. MCP servers also work. | https://learn.chatgpt.com/docs/app-server |
| `codex exec --json`, `codex mcp-server` | Non-interactive run with a JSONL event stream, and Codex as an MCP server. | Via MCP config. | UNVERIFIED in this pass. My fetch of the exec docs returned 404. |

Key quote on stability (FACT, https://learn.chatgpt.com/docs/app-server): the app-server command and WebSocket transport "aren't supported for production workloads". The doc recommends the SDK for automation and CI, and app-server for deep product integration (auth, history, approvals, streamed events).

**App-server with the official plan-usage tokens (FACT, https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server):** your app runs the OAuth flow, then launches `codex app-server --listen stdio://` with `ACCESS_TOKEN` in the child environment. The provider config points at it with `env_key="ACCESS_TOKEN"`, and `supports_websockets=false` forces HTTP. "No separate Codex sign-in is required." Codex sends the bearer to `/v1/responses`. To renew, restart app-server with the new token, re-run `initialize`, and call `thread/resume`. Count only `turn/completed` with status `completed` as success. Configurations that emit Responses `tool_search` will fail.

**INFERENCE:** Codex app-server is a workable second engine: it has its own loop, tools, sandbox, and approvals, and it can run on Mac, Windows, and Linux (Windows support not verified here). Cost: a Rust binary per device, an experimental protocol that changes with each Codex release (generate types per installed version), and a second tool-permission model.

### B5. How existing open tools do it

- **Pi (FACT).** Two OpenAI OAuth providers: `openai` ("Sign in with ChatGPT: uses the ChatGPT subscription with the OpenAI API") and `openai-codex` (legacy, API id `openai-codex-responses`, GPT-5.x Codex models, transports `sse`, `websocket`, or `auto`, WebSocket reuse per session with a 5-minute idle expiry, session-based prompt caching). Login runs through `provider.auth.oauth` (`login`, `refresh`, `toAuth`), with token refresh serialized under a credential-store lock. Credentials live in pi's `auth.json`. A headless user can paste the redirect URL or code back into Pi. A plain `openai` provider with OAuth hides Decisions API models because the endpoint "needs an OpenAI API key; Sign in with ChatGPT credentials are rejected". (https://github.com/earendil-works/pi/blob/main/packages/ai/README.md, https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/providers.md)
- **OpenCode (SECONDARY).** Native Codex login since v1.1.11, via `/connect`. I could not read its source in this pass. (https://blog.devgenius.io/jump-ship-in-minutes-codex-oauth-now-works-in-opencode-d2708c32f571)
- **Others (SECONDARY).** Community plugin `opencode-openai-codex-auth`, OpenHands subscription docs (https://docs.openhands.dev/sdk/guides/llm-subscriptions.md), assistant-ui (https://www.assistant-ui.com/docs/guides/chatgpt-subscription.md), Pydantic AI (https://pydantic.dev/docs/ai/models/openai-codex/). All reuse the Codex CLI flow. UNVERIFIED which of them have moved to the official plan-usage flow.

### B policy risks

1. **Legacy path risk.** Calling `chatgpt.com/backend-api/codex` with the Codex CLI client id (or copying `~/.codex/auth.json` into your app) is not the documented path for third parties. No primary OpenAI text permits it. It can break or be blocked at any time. INFERENCE. Do not build on it.
2. **Official path is a program, not an open API.** Open-source and locally hosted apps are the stated audience. A hosted or paid app needs the interest form and is a "limited trial". If "cloud tasks" means infrastructure that you run for other users, you are on the commercial side.
3. **Preview limits.** Docs call this a preview (page "Preview limitations"). Features such as hosted tools are missing. Expect change.
4. **Workspace binding and no revocation for transferred sessions.** Copying credentials to a VM works, but per-host revocation and attribution are "not yet available".
5. **Plan scope.** Only Plus and Pro are named as eligible. Business and Enterprise are not mentioned, and an admin may block device-code login.
6. **Secrets handling.** Docs require owner-only file permissions, and no commit or log of tokens. Your app needs an encrypted credential store, not plain JSON.
7. **OpenAI Terms of Use** not read. UNVERIFIED.
8. **Codex app-server** is "not supported for production workloads".

---

## C. Fireworks AI

### C1. API

- **FACT** OpenAI-compatible base URL: `https://api.fireworks.ai/inference/v1`. Chat completions and completions are shown. The compatibility page does not mention a Responses endpoint or embeddings. (https://docs.fireworks.ai/tools-sdks/openai-compatibility)
- **FACT** Anthropic-compatible endpoint exists: SDK base URL `https://api.fireworks.ai/inference` (no `/v1`, the SDK appends `/v1/messages`). Auth header `Authorization: Bearer $FIREWORKS_API_KEY`. Model ids are Fireworks resource names such as `accounts/fireworks/models/kimi-k2p5`. Anthropic model names are not accepted. (https://docs.fireworks.ai/tools-sdks/anthropic-compatibility.md)
  - Supported: `POST /v1/messages` (stream and non-stream), serverless and on-demand, thinking (`output_config.effort` or `thinking`), images, structured output, client-side tools (including tool search and deferred loading), streaming usage, `service_tier: "priority"`, `context_management`.
  - Not supported: `count_tokens` (404), batches, server tools (code execution, memory, web fetch, server-side web search), tool-schema fields `cache_control`, `eager_input_streaming`, `allowed_callers`, `input_examples`, `output_config.speed`, `document` blocks. `cache_creation_input_tokens` is always `0`.
- **FACT** Pi lists Fireworks as a provider that "uses OpenAI- and Anthropic-compatible APIs" with env var `FIREWORKS_API_KEY`. (https://github.com/earendil-works/pi/blob/main/packages/ai/README.md)

### C2. Models, context, prices (serverless, USD per 1M tokens: input / cached input / output)

Prices: **FACT**, https://docs.fireworks.ai/serverless/pricing (standard tier). Context lengths: **FACT** from https://fireworks.ai/models and https://fireworks.ai/models/fireworks/kimi-k3 where shown, otherwise "not shown". Batch is 50% of serverless. Priority tier costs about 25% more.

| Model | Context | Price in / cached / out | Notes |
|---|---|---|---|
| Kimi K3 (`accounts/fireworks/models/kimi-k3`) | 1,040k ("1M") | 3.00 / 0.30 / 15.00 | Function calling "Supported". Weights released 2026-07-27 per model page. Fast router `accounts/fireworks/routers/kimi-k3-fast` (4.50 / 0.45 / 22.50). US-only router `.../routers/kimi-k3-us` (4.50 / 0.45 / 22.50). |
| GLM 5.3 (slug `fireworks/glm-5p3`) | 1,048,576 on models page | 1.40 / 0.26 / 4.40 | The training price table lists 262K for GLM 5.3. Sources disagree, confirm before you size prompts. |
| GLM 5.3 Flash | 1,048,576 | 0.15 / 0.03 / 0.50 | Vision. |
| DeepSeek V4.1 Flash (`deepseek-ai/deepseek-v4p1-flash`) | 1,048,576 | 0.30 / 0.006 / 1.20 | Vision. Very cheap cached input. |
| Qwen 3.8 Max | not shown | 2.00 / 0.25 / 6.00 | |
| Qwen3.8 Flash Next (FP8, FP4) | 262,144 | not in price table | No serverless tag shown on models page. |
| MiniMax M3 | not shown | 0.30 / 0.06 / 1.20 | Models page also lists MiniMax M3.1 Flash, 1,048,576 context. |
| gpt-oss-120b | not shown | 0.15 / 0.015 / 0.60 | |
| Nemotron 3 Ultra (preview) | not shown | 0.60 / 0.12 / 2.40 | |

**UNVERIFIED:** which of these is best at agentic tool use. I did not find a Fireworks ranking and did not run a benchmark. The models page showed only 9 of 329 models. Kimi K2.x, Qwen3-Coder, and GLM 4.x are not in the pricing page I read, so they may be retired or renamed. Check `GET /v1/models` on your account.

### C3. Tool calling, reasoning, streaming, caching

All **FACT** unless marked.

- **Tool calling** (https://docs.fireworks.ai/guides/function-calling): `tool_choice` supports `auto` (default), `none`, `required`, or a named function. Parallel calls work on some models only, so check model capabilities. Check the model's `supportsTools` field. Arguments stream incrementally and you accumulate them per tool-call `index` until `finish_reason == "tool_calls"`. Arguments "can be malformed or partial JSON", so validate before you parse. Tool names are limited to 64 characters (letters, digits, underscore, dash). 400 errors come from external `$ref` URIs or bad fragment paths. Low temperature (0.0 to 0.3) is recommended for tool selection.
- **Reasoning** (https://docs.fireworks.ai/guides/reasoning): reasoning comes back in `message.reasoning_content` and in streamed `delta.reasoning_content`, separate from `content`. For some models it can still sit inside `content`. `reasoning_effort` takes `low`, `medium`, `high` (other values unconfirmed). The Anthropic-style `thinking: {type: "enabled", budget_tokens: N}` needs N of at least 1024. "You cannot specify both `thinking` and `reasoning_effort` in the same request."
- **Multi-turn tool use:** interleaved thinking needs you to send back the previous assistant `reasoning_content` when the last message has role `tool`. If you drop it, the model loses its earlier reasoning and there is no error. `reasoning_history="preserved"` keeps reasoning from earlier user turns. Tool-call turns can have empty `content`. Per-model formats differ (Kimi K2 Thinking wraps reasoning in `<think>`, MiniMax M2 and GLM 4.7 use their own tool-call markup). The page does not cover DeepSeek, gpt-oss, or Qwen. INFERENCE: your adapter must store `reasoning_content` per assistant message and replay it.
- **Streaming:** `usage` arrives in the last chunk (the one with `finish_reason`). The OpenAI SDK types omit it, so read it from the raw chunk. (https://docs.fireworks.ai/tools-sdks/openai-compatibility)
- **Context overflow:** default `context_length_exceeded_behavior` lowers `max_tokens` to fit ("truncate"). Set `error` for OpenAI-like behavior. (same URL)
- **Prompt caching** (https://docs.fireworks.ai/guides/prompt-caching): "enabled by default for all Fireworks models and deployments". Longest matching prefix is reused. Cached tokens on serverless are discounted, "typically 50% but varying by model" (the price table shows deeper discounts for some models, for example DeepSeek V4.1 Flash). Keep related requests on one replica with the `x-session-affinity` header or the `user` field. Use `x-prompt-cache-isolation-key` (or `prompt_cache_isolation_key`) to separate cache scopes. Minimum prefix length is not stated. Over the OpenAI endpoint the page does not name the `cached_tokens` usage field (UNVERIFIED).

### C policy risks

1. Fireworks terms of service and acceptable-use policy not read. UNVERIFIED.
2. Data handling: Fireworks serves open-weight models. "US-only" variants cost about 10% to 50% more. If data residency matters, use the `(US)` router ids. Prices and model lines change fast.
3. The key is a plain API key. Policy risk is low compared with A and B. Real risks are operational: silent reasoning loss, malformed tool JSON, and model churn.

---

## Integration options

| Provider | Path | Pros | Cons |
|---|---|---|---|
| Anthropic | **Agent SDK engine** (one `query()` subprocess per task; our tools through `createSdkMcpServer`; `tools: []`) | Full Claude Code agent loop, sessions with resume and fork, hooks, `canUseTool`, subagents, cost reporting. Ships a native binary for Mac, Windows, and Linux. Official docs for every piece. | One subprocess (about 230 to 260 MB when warm) per task. Second permission and tool model next to Pi. Fast release pace (daily patches). Plan login not allowed for a distributed app, so API key, Bedrock, Vertex, or Foundry only. |
| Anthropic | **Pi provider** (Pi's Anthropic provider with an API key) | One loop, one tool system, one event model. Lighter. | Loses Claude Code features (built-in tools, skills, compaction, subagents). Pi's Anthropic OAuth path must stay off. |
| Anthropic | Plain HTTP (Messages API) | Smallest dependency, you own the loop. | You rebuild everything the SDK gives you. |
| OpenAI | **Pi provider** (`openai` provider with Sign in with ChatGPT, official plan usage) | Official path for open-source and local apps. One loop. Pi already implements login, locked refresh, and a credential store. | Preview limits (no hosted tools, `store: false`, `stream: true`, no `previous_response_id`). Plus and Pro only named. Hosted or paid use needs OpenAI approval. Pi is Node 22.19 or newer. |
| OpenAI | **Codex app-server engine** (stdio JSON-RPC; `ACCESS_TOKEN` env from the official flow) | Full Codex agent loop with its own sandbox, approvals, and thread resume. Dynamic tools and MCP for our tools. | "Not supported for production workloads". `dynamicTools` is experimental. Rust binary per device. Types must match the installed version. Token renewal needs an app-server restart plus `thread/resume`. |
| OpenAI | `@openai/codex-sdk` | Simple `startThread` and `run`. | Custom tools not documented. Process model not documented. Auth path for plan tokens unverified. |
| OpenAI | Plain HTTP (`POST api.openai.com/v1/responses` with the plan token) | Small and explicit. Works with our own loop. | You must build OAuth, registration, refresh, and error handling yourself. Same preview limits. |
| OpenAI | Legacy Codex path (`chatgpt.com/backend-api/codex`, Codex client id) | Works today in many tools (SECONDARY). | Not documented for third parties. Pi marks it "legacy". Can break or be blocked. Do not use for a distributed app. |
| Fireworks | **Pi provider** (OpenAI chat completions, `FIREWORKS_API_KEY`) | Already built in Pi. Simple API-key auth. Cheap models, 1M-class context on some. Automatic prompt caching. | Must replay `reasoning_content`. Malformed tool JSON needs validation. Model churn. |
| Fireworks | Plain HTTP via the Anthropic-compatible `/v1/messages` | Reuses Anthropic message shapes and thinking blocks. | No server tools, no `count_tokens`, tool `cache_control` unsupported. |
| Fireworks | Agent SDK with `ANTHROPIC_BASE_URL` | Gives Claude Code tooling on open models. Fireworks documents the setup. | **Unsupported by Anthropic** ("doesn't support routing Claude Code to non-Claude models through any gateway"). Fragile. Not recommended as a day-one path. |

**Suggested day-one shape (INFERENCE):**

1. Define an `Engine` interface in the app: `start(task)`, `send`, `interrupt`, `resume`, plus a normalized event stream and a normalized permission request.
2. Engine 1: **Pi** for OpenAI (official Sign in with ChatGPT plan usage) and Fireworks (API key).
3. Engine 2: **Agent SDK** for Anthropic, API key first. Treat "use my existing `claude` login" as a personal-use toggle that never reads or stores the token.
4. Defer Codex app-server until you need Codex's own loop.
5. Put a per-provider "policy status" in the app so a user sees which auth mode they use.

---

## Open gaps (re-check before you build)

- OpenAI Terms of Use, Fireworks terms, and Anthropic Consumer Terms not read.
- OpenAI DevDay recap page returned 403. Announcement details are SECONDARY.
- Codex CLI OAuth `client_id`, literal account-id header value, `codex exec --json`, and `codex mcp-server` not verified. Codex SDK custom tools and process model not documented on the page I read.
- OpenCode's implementation source not read.
- `AgentDefinition` fields and Windows prerequisites for Claude Code not read. Windows support for the Agent SDK is INFERRED from the `win32-x64` package.
- No benchmark of Fireworks models for agentic quality.
- Anthropic's April and May 2026 enforcement events come from news sources only.
