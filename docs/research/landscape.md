# Landscape: Claude Duet and multi-device agent products

Date of research: 2026-10-28. Read-only. No sign-ups, no logins.

## How to read this report

- **FACT** means a source states it. The URL follows the claim.
- **INFERENCE** means I reasoned from facts. The reasoning is short and visible.
- Page text came through a fetch tool that summarizes pages with a small model. Text inside quotation marks matches the tool output, not necessarily the raw page. Check any quote you plan to publish.
- Star counts, fork counts, and commit counts are the numbers on the page at fetch time (2026-10-28). Last-commit dates were not visible on most GitHub pages.
- I did not run or read the code of any project. Fork-base notes come from READMEs and docs only.
- Two OpenAI pages (the launch post and one help article) returned HTTP 403. For those I used the OpenAI developer page that did load, and search snippets from the official domains.

## Top findings

1. **FACT.** Official Claude Duet pages do not describe channels with their own memory notes, task list, routines, files folder, purpose line, or GitHub repos. They say channels "share the same memory". The shape you describe is much closer to **Claude Code Projects** (public beta). See Part 1.4.
2. **FACT.** Claude Code Projects already has a first-party version of your idea: cloud threads by default, and a thread on the user's own computer through Remote Control when the task needs it. It is single-machine, Anthropic-only, and runs only while the computer is awake.
3. **FACT.** OpenAI Dots allows one connected personal computer at a time. Users ask in the OpenAI forum for several computers with per-device permissions and a preferred device. This is the gap you want to fill.
4. **FACT.** The strongest "one agent, many machines" designs use an outbound-only daemon on each device (Omnara `omnarad`, OpenClaw node host, Claude Code Remote Control, Codex relay). No inbound ports. The hub holds state and policy.
5. **FACT.** CopilotKit OpenDuet has the right shape (channels, tasks, routines, approvals, MIT license, AG-UI). It depends on the hosted CopilotKit Intelligence service for threads and memory, and its "computers" are Docker containers on the server host, not your devices.

---

## Part 1: Claude Duet (official sources)

### 1.1 What it is

- **FACT.** "Claude Duet is a research preview of Claude that you message the way you'd message a person." It "works on its own computer in the cloud, so it can keep working after you close the app." https://support.claude.com/en/articles/17631495-get-started-with-claude-duet
- **FACT.** Product page: "Claude Duet is a proactive agent that understands what needs your attention. It takes on tasks of all sizes, keeps working while you're away, and waits for your ok when it matters." https://claude.com/product/claude-duet
- **FACT.** Launch date in the release notes: October 21, 2026. https://support.claude.com/en/articles/12138966-release-notes (A search summary says some pages list October 23. Not resolved.)
- **FACT.** Model: "Claude Duet runs on Claude Opus 5.5 and decides how much effort a job needs." The user cannot choose a model. https://support.claude.com/en/articles/17631495-get-started-with-claude-duet
- **FACT.** Surfaces: standalone app for iOS and Android, plus claude.ai on a computer. https://claude.com/product/claude-duet and https://claude.com/resources/articles/claude-duet
- **FACT.** Availability: research preview, waitlist, US Pro and Max only. Not on Team or Enterprise yet. Included at no extra cost. Uses the same five-hour and weekly limits as chat, and background work counts. https://support.claude.com/en/articles/17631495-get-started-with-claude-duet

### 1.2 Channels, tasks, files

- **FACT.** The app has three tabs: Chat (one ongoing thread plus optional channels), Tasks, Make. Source is a secondary newsletter that paraphrases the help article. https://roo.beehiiv.com/p/claude-duet-explained
- **FACT.** Channels: "start separate channels for different topics, and they share the same memory." The main thread persists, so "Claude Duet picks up where you left off." https://support.claude.com/en/articles/17631495-get-started-with-claude-duet
- **FACT.** Tasks: "The **Tasks** tab shows what Claude Duet is working on and highlights tasks that need your input." The main thread posts when a task finishes or needs a decision. Duet may suggest a task is done, but "only you can mark a task as completed." https://support.claude.com/en/articles/17631495-get-started-with-claude-duet
- **FACT.** Larger results arrive as editable files in the Make tab. https://support.claude.com/en/articles/17631495-get-started-with-claude-duet
- **FACT.** Product page: "Send a message and it sets up a meeting or builds a dashboard, then shares when it's done." Large tasks "require more model turns and background work." https://claude.com/product/claude-duet
- **FACT.** Not documented in the Duet pages I read: per-channel memory, per-channel task list, per-channel files folder, a purpose line, or attached GitHub repos. The Duet product and launch pages do not mention GitHub, except that testers use it to "ship code". https://claude.com/resources/articles/claude-duet
- **FACT.** The release notes have no entry that mentions channels, or channels with notes, task lists, files, or repos. https://support.claude.com/en/articles/12138966-release-notes

### 1.3 Routines, morning brief, memory

- **FACT.** Morning brief: "Each morning, it looks across your email and calendar, picks what most needs your attention." It can start first steps, such as drafting replies for review. https://support.claude.com/en/articles/17631495-get-started-with-claude-duet
- **FACT.** "Claude Duet checks your connected accounts every few hours," to protect plan limits. It messages you only when something needs attention. You can tell it what to report and how often. https://claude.com/resources/articles/claude-duet
- **FACT.** The help article does not describe user-defined routines by name. The closest features are the brief and the check-ins. https://support.claude.com/en/articles/17631495-get-started-with-claude-duet
- **FACT.** Memory: "Your memory, skills, and apps from the Claude app are there from the first message, all in one thread." Duet can search past chats (setting: "Search and reference chats"). Disconnecting an app keeps memory of earlier information. https://claude.com/product/claude-duet and https://support.claude.com/en/articles/17631495-get-started-with-claude-duet
- **FACT.** Duet "keeps your conversation history and its memory". Deleting the account removes everything it stored. Duet cannot store credentials in memory. https://claude.com/resources/articles/claude-duet-safety-privacy-and-security

### 1.4 Nearest official analogue to your channel spec: Claude Code Projects

- **FACT.** "A project is one ongoing conversation where Claude coordinates a stream of related work for you. You tell it what needs doing and it starts a thread for each task." Each thread "is usually a cloud session". Status: public beta on Pro and Max, not on Team or Enterprise. https://code.claude.com/docs/en/claude-projects
- **FACT.** Threads are workers. Each has "its own context window", does one piece of work, and "reports back to the conversation when it finishes". The coordinator "sees what threads report back, not every step they take." https://code.claude.com/docs/en/claude-projects
- **FACT.** Project memory is "Notes Claude keeps about the project... stored as files." Every cloud thread reads the index file `MEMORY.md` at start. Project instructions are text up to 16,000 characters. https://code.claude.com/docs/en/claude-projects
- **FACT.** Project parts: repositories, a Library tab (files you add and files threads produce, readable under `/mnt/project-files`), a Pull requests tab, a Scheduled tasks tab, and a "Waiting on you" view. https://code.claude.com/docs/en/claude-projects
- **FACT.** Library limits: up to 100 files and 2 GB in one pick, a single file up to 480 MB. https://code.claude.com/docs/en/claude-projects
- **FACT.** Cloud threads do not get the machine's skills, MCP servers, plugins, or tools, and they do not apply `.claude/settings.json` permission rules or hooks. https://code.claude.com/docs/en/claude-projects
- **FACT.** "When a task needs something only your computer has, such as a local database, a device emulator, or an API behind your VPN, ask Claude to run the thread for that task on your computer instead of in the cloud." The thread is a Claude Code session in a folder on your machine, connected through Remote Control. It "runs only while that computer is awake". It needs Claude Code v2.1.280 or later. It does not work while "Require trusted devices" is on. https://code.claude.com/docs/en/claude-projects
- **FACT.** Threads run in auto mode when the model supports it. Approval prompts live inside the thread. "Telling Claude in the project conversation to go ahead doesn't reach it." https://code.claude.com/docs/en/claude-projects
- **FACT.** Project controls: Pause, Archive, Delete. A scheduled thread that hits a usage limit stops instead of waiting. https://code.claude.com/docs/en/claude-projects
- **FACT.** A sibling feature, Cowork projects, scopes memory per project: "What Claude learns in one project doesn't carry over to other projects." A toggle, "Use account memory", keeps a project's memory separate. https://support.claude.com/en/articles/14116274-organize-your-tasks-with-projects-in-claude-cowork
- **INFERENCE.** Your "Everything chat plus channels" is Duet's front end. Your "channel with notes, tasks, routines, files, purpose, repos" is the Projects data model. You are combining the two. Neither official product documents that combination.

### 1.5 Connectors, approvals, safety

- **FACT.** "Works with more than 6,000 connectors and plugins." Examples: Gmail, Slack, Google Calendar, Google Drive, Notion, Stripe, QuickBooks, HubSpot, Docusign. https://claude.com/product/claude-duet and https://support.claude.com/en/articles/17631495-get-started-with-claude-duet
- **FACT.** Sites without a connector go through Duet's cloud browser. You "can't watch or take over the browser." https://support.claude.com/en/articles/17631495-get-started-with-claude-duet
- **FACT.** Approvals: "it shows you exactly what it's about to do and waits for your approval" before it sends a message, makes a payment, signs a document, or deletes anything. If you cannot approve now, it finishes the rest and asks later. "A few things are off-limits even with your approval, like trading securities or moving money between accounts." https://claude.com/resources/articles/claude-duet and https://support.claude.com/en/articles/17631495-get-started-with-claude-duet
- **FACT.** A separate check, built on the Claude Code auto mode classifier, "assesses whether the action matches your intent". It "runs outside the model doing the work so instructions hidden in a page cannot affect the outcome." https://claude.com/resources/articles/claude-duet-safety-privacy-and-security
- **FACT.** Credentials: "Claude Duet never sees your passwords, browser cookies, sign-in tokens, or other credentials." For connected services, a placeholder sits in the agent's environment and "a proxy outside it inserts the real value at request time." https://claude.com/resources/articles/claude-duet-safety-privacy-and-security
- **FACT.** "Your browser in Claude Duet runs in its own dedicated virtual machine in the cloud, separate from the model." "The agent drives the browser only through a fixed set of commands." https://claude.com/resources/articles/claude-duet-safety-privacy-and-security
- **FACT.** A sensitive-information classifier checks each outgoing message. Strict tier (credentials, full card numbers, government IDs) is never shared. https://claude.com/resources/articles/claude-duet-safety-privacy-and-security
- **FACT.** The help article warns that prompt injection is a risk and "no defense is perfect." https://support.claude.com/en/articles/17631495-get-started-with-claude-duet
- **FACT.** Duet "can't see all projects or files on your computer", cannot call or text, and can send email only with approval. No documentation of local devices or MCP appeared in the safety article. https://support.claude.com/en/articles/17631495-get-started-with-claude-duet and https://claude.com/resources/articles/claude-duet-safety-privacy-and-security

### 1.6 Mobile

- **FACT.** iOS and Android apps. Setup: accept the invitation email on the phone, use the Apple or Google account tied to the waitlist email. https://support.claude.com/en/articles/17631495-get-started-with-claude-duet

---

## Part 2: Similar products

### 2.1 CopilotKit OpenDuet (https://github.com/CopilotKit/OpenDuet)

- **FACT.** "An open-source proactive agent that gets your work done and keeps you in the loop." Built on OpenBot, CopilotKit's agent platform. License MIT. About 1.4k stars, 166 forks, 73 commits on `main`. README calls it early and in active development. https://github.com/CopilotKit/OpenDuet
- **FACT.** Stack: Bun 1.3+, Node, Docker, PostgreSQL with pgvector, Hono server with CopilotKit runtime, Chromium in per-agent containers, MCP connectors (Google Drive, Notion; Composio brokers more). Default model is OpenAI `gpt-5.6-terra`; Anthropic via `BOT_PROVIDER`; any OpenAI-compatible endpoint via `OPENAI_BASE_URL`. https://github.com/CopilotKit/OpenDuet
- **FACT.** Channels: web app, Slack, Microsoft Teams. A channel is "a named work area with a colour and a purpose"; tasks started there take its colour. Everything "stays your one thread". https://github.com/CopilotKit/OpenDuet
- **FACT.** Tasks: one Tasks tab. Statuses: Working, Ready for review, Needs input, Resolved. A failed run shows the reason and can offer a retry. https://github.com/CopilotKit/OpenDuet
- **FACT.** Routines: "scheduled turns that run as the user", managed at `/routines`. A worker process runs them. https://github.com/CopilotKit/OpenDuet
- **FACT.** Memory: "Threads and memory live in CopilotKit Intelligence". Users can see and correct what it remembers. https://github.com/CopilotKit/OpenDuet
- **FACT.** Hosted dependency: CopilotKit Intelligence is required. "The server will not start without its key." The free Developer plan is enough. Setup uses `npx copilotkit login`. Telemetry goes to CopilotKit by default; opt out with `COPILOTKIT_TELEMETRY_DISABLED=true` or `DO_NOT_TRACK=1`. https://github.com/CopilotKit/OpenDuet
- **FACT.** The architecture doc lists Intelligence as "external" ("Durable threads, memory, and realtime gateway"). "Each channel maps to an Intelligence thread" and "An Intelligence thread admits exactly one agent." The doc does not say whether Intelligence can be self-hosted. https://raw.githubusercontent.com/CopilotKit/OpenDuet/main/openbot/docs/architecture.md
- **FACT.** AG-UI: the server "sends it to a Bot over AG-UI". The CopilotKit runtime forwards each turn to the configured AG-UI endpoint and registers frontend tools (browser, MCP, components). A Bot at its own endpoint can call back to `message_bot` or `ask_person` through a signed callback route. The README says any AG-UI-compatible agent works. https://raw.githubusercontent.com/CopilotKit/OpenDuet/main/openbot/docs/architecture.md and https://github.com/CopilotKit/OpenDuet
- **FACT.** Sandbox: each Bot gets a container with Chromium, `/workspace`, a browser profile, and a shell. `agent-computer` listens on HTTP port 4100 and needs a `COMPUTER_TOKEN` on every route except `/health`. A `supervisor` holds the Docker socket and exposes only ensure, stop, reset, list. https://raw.githubusercontent.com/CopilotKit/OpenDuet/main/openbot/docs/architecture.md
- **FACT.** Policy: "The computer itself does not decide policy. The server gateway is the action boundary." Gateway order: resolve target, evaluate policy (CEL rules, deny before allow, fails closed), write an audit row, then act. https://raw.githubusercontent.com/CopilotKit/OpenDuet/main/openbot/docs/architecture.md
- **FACT.** Network limits: each computer runs a filter proxy with modes `allow_all`, `defaults_plus_allowlist`, `allowlist_only`, `deny_all`. "Cloud metadata addresses are refused in every mode." Limit: processes that ignore proxy variables and open raw sockets are not stopped; "that is a Kubernetes NetworkPolicy's job." https://raw.githubusercontent.com/CopilotKit/OpenDuet/main/openbot/docs/architecture.md
- **FACT.** The README describes no device control beyond the containerized computer. The deployment doc says OpenDuet "has no published image or deploy guide yet". https://github.com/CopilotKit/OpenDuet and https://github.com/CopilotKit/OpenDuet/tree/main/openbot
- **Fork-base assessment (INFERENCE).**
  - Good: right product shape; MIT; AG-UI makes the agent swappable; one policy gateway with audit; egress proxy; per-Bot computers.
  - Bad: hosted Intelligence dependency for threads and memory; unclear self-hosting; the "channel = one thread" mapping conflicts with the README claim of one thread; computers run on the server host only; early (73 commits); Bun, Docker, Postgres, and Helm weight.
  - Seam for devices: `agent-computer` is an HTTP service with a token. A device daemon that implements the same API, reached over an outbound tunnel, could replace it. I did not read the code, so treat this as a hypothesis.
  - Cut Intelligence and keep the rest if you fork. Expect to rewrite the thread and memory layer.

### 2.2 Anil-matcha/open-duet (a different "Open Duet", found during search)

- **FACT.** Apache-2.0. About 6.3k stars, 791 forks, 419 commits. README calls it "an open alternative to Claude Duet and to other paid AI agent products such as OpenAI Dots, Meta Muse, Grok Bot..." and "not yet a feature-for-feature replacement". https://github.com/Anil-matcha/open-duet
- **FACT.** Named agents with instructions, engine and model, workspace folder, approval mode. Per-agent memory (`MEMORY.md` plus topic notes). Routines (cron, interval, one-off, webhook) with run history. A task board and calendar. Approval modes: Ask first, Accept edits, Plan only. https://github.com/Anil-matcha/open-duet
- **FACT.** Engines: Claude Code, Codex CLI, Gemini CLI, Cursor, ACP-compatible CLI agents, OpenRouter, OpenAI-compatible APIs (Ollama, LM Studio). A turn can fall back to another engine when a usage limit hits. Team leads must use Claude or Codex. https://github.com/Anil-matcha/open-duet
- **FACT.** Isolated Docker "computer" per agent (experimental, off by default). Server is "intended for one owner with paired devices". A phone app pairs with your server; tested only on an Android emulator; no push notifications. The page does not say agents can run on remote machines. https://github.com/Anil-matcha/open-duet
- **INFERENCE.** Useful as a reference for ACP engines, limit-aware fallback, and routine run history. It does not solve multi-device execution.

### 2.3 OpenAI Dots

- **FACT.** "Dots are always-on agents that keep ongoing work moving and ask you for decisions when needed." Each dot runs "on its own cloud computer and browser" and "can keep working even when your computer is off." Powered by GPT-6 Astra. https://learn.chatgpt.com/docs/dots
- **FACT.** Local execution: "You can connect one personal computer through the desktop app." Its files, code, and apps then become available. The computer "must be online with the ChatGPT app open". "Only one personal computer can be connected at a time." https://learn.chatgpt.com/docs/dots
- **FACT.** A dot can run background agents in parallel, start cloud threads, and create Work or Codex tasks on the connected computer. https://learn.chatgpt.com/docs/dots
- **FACT.** Plugins: "over 4,000 apps" (launch coverage; the official page says supported plugins enabled for your account). Website logins use a private sign-in flow; the cloud browser keeps its own sessions. https://learn.chatgpt.com/docs/dots and https://openai.com/index/introducing-dots/ (search snippet; page returned 403)
- **FACT.** Memory: conversation, ChatGPT memory, and the dot's "own saved notes". Research while idle is read-only. Custom Rules add limits; "Neither instructions nor custom rules override built-in safety requirements." https://learn.chatgpt.com/docs/dots
- **FACT.** Scheduling: dots decide when to pause and resume; recurring tasks sit under "Scheduled" in the desktop app. Dot conversations do not count against ChatGPT limits; tasks a dot runs in Work or Codex do. https://learn.chatgpt.com/docs/dots
- **FACT.** Access: Pro 100, 200, and 500 tiers outside the EEA, UK, and Switzerland; Business Premium; Enterprise off by default. https://learn.chatgpt.com/docs/dots
- **FACT.** Forum requests: several authorized computers at once, "per-device permissions and a preferred computer/fallback setting", choosing the computer per task, and pause or revoke per device. https://community.openai.com/t/let-one-dot-access-multiple-authorized-computers/1402719
- **INFERENCE.** The Dots forum thread is a ready-made requirements list for your device feature.

### 2.4 Spacebot, ZeroClaw, Goose, OpenClaw, Hermes Agent

**Spacebot** (https://github.com/spacedriveapp/spacebot)
- **FACT.** Rust single binary. License FSL-1.1-ALv2, which converts to Apache 2.0 after two years (source-available, not open source today). About 2.4k stars. README says it is in beta; line 0.6. https://github.com/spacedriveapp/spacebot
- **FACT.** Process types: channels (conversation; "no shell, no filesystem, no memory search"), branches (fork a channel's context to think), workers (execute tasks), a Compactor, and a supervisor. Shell tools run under bubblewrap (Linux) or sandbox-exec (macOS). Workers can run OpenCode. https://github.com/spacedriveapp/spacebot
- **FACT.** Multi-device: "workers can target any device in your library", but only through optional pairing with Spacedrive. https://github.com/spacedriveapp/spacebot
- **FACT.** Four-level model routing per call type; any OpenAI-compatible or Anthropic-compatible endpoint; Ollama. Tasks have dependency graphs; autonomous work waits in `pending_approval`; cron jobs auto-disable after 3 consecutive failures. Memory in SQLite and LanceDB with typed, scored, graph-linked entries. https://github.com/spacedriveapp/spacebot

**ZeroClaw** (https://github.com/zeroclaw-labs/zeroclaw)
- **FACT.** Rust, "a single Rust binary you configure and run". Dual license MIT or Apache-2.0. About 33.1k stars, 5,761 commits. https://github.com/zeroclaw-labs/zeroclaw
- **FACT.** Runs on your machine; `service install` registers systemd, launchctl, or Windows Service. HTTP and WebSocket gateway with dashboard; ACP over stdio for editors. 30+ channel adapters. About 20+ providers with fallback chains. Default autonomy is "supervised". Sandboxes: Landlock, Bubblewrap, Seatbelt, Docker. Memory: SQLite with embeddings. https://github.com/zeroclaw-labs/zeroclaw
- **FACT.** The page does not describe multi-node operation. https://github.com/zeroclaw-labs/zeroclaw

**Goose** (https://github.com/aaif-goose/goose)
- **FACT.** The page says "goose is part of the Agentic AI Foundation (AAIF) at the Linux Foundation." It does not mention Block. Rust, Apache-2.0, about 55.9k stars, 5,899 commits. Desktop app, CLI, and API. https://github.com/aaif-goose/goose
- **FACT.** "Connect to 70+ extensions via the Model Context Protocol". "15+ providers". ACP lets you use "your existing Claude, ChatGPT, or Gemini subscriptions". The page does not say whether goose is an ACP client, server, or both. It does not describe remote or multi-device execution. https://github.com/aaif-goose/goose

**OpenClaw** (https://github.com/openclaw/openclaw)
- **FACT.** MIT. About 393k stars, 121,830 commits, about 5k open issues, 2.8k open PRs. Stewarded by the OpenClaw Foundation (501(c)(3)). Gateway is "the local control plane for sessions, tools, events, and channel connections". 20+ chat channels. Models are swappable plugins. https://github.com/openclaw/openclaw
- **FACT.** Tools run on the host for the main session unless sandboxing is configured. Inbound messages are untrusted; unknown DM senders must pair. https://github.com/openclaw/openclaw
- **FACT.** Nodes are covered in 2.5. https://docs.openclaw.ai/nodes
- **INFERENCE.** The issue and PR counts match your "sloppy" feel: very high churn. The node design is still worth studying.

**Hermes Agent** (https://github.com/NousResearch/hermes-agent)
- **FACT.** MIT. About 258.1k stars, about 60k commits. "Six built-in terminal backends: local, Docker, SSH, Modal, Daytona, and Vercel Sandbox." Gateway for Telegram, Discord, Slack, WhatsApp, Signal, CLI. "Use any model you want" (Nous Portal, OpenRouter, OpenAI, custom endpoint). Isolated subagents, natural-language cron, agent-curated memory and auto-created skills. https://github.com/NousResearch/hermes-agent
- **INFERENCE.** The SSH backend means the terminal can sit on another host. The page does not say one agent can use several backends at once. I did not verify this.

### 2.5 Projects that already do "one agent, many machines"

| Project | Transport | Auth and pairing | NAT traversal | Tool routing | Models | License |
|---|---|---|---|---|---|---|
| Claude Code Remote Control | Outbound HTTPS only. Registers with the Anthropic API and polls for work. Streaming connection relays messages. | claude.ai subscription auth only (no API keys). Several short-lived scoped credentials. Optional Trusted Devices. | Not needed. "Never opens inbound ports." | The whole Claude Code session runs on the device. The web and phone are a window. | Anthropic only. Blocked if `ANTHROPIC_BASE_URL` is not `api.anthropic.com`. | Proprietary |
| Codex Remote | Relay layer. Hosts reachable "rather than directly from the internet". SSH hosts: SSH starts a remote Codex app server. | QR code on the host pairs one phone with one host. Same account and workspace. | Relay | The host runs commands, reads files, writes changes. | OpenAI | Proprietary |
| OpenAI Dots local | Via ChatGPT desktop app, app must be open | Connect from the dot's profile | Not stated | Dot starts Work or Codex tasks on the computer | OpenAI | Proprietary |
| OpenClaw node host | WebSocket to the Gateway, `role: "node"`. Loopback Gateway needs an SSH tunnel. Watch node uses signed HTTPS polling. | Two-stage approval: device (`openclaw devices approve`), then command surface (`openclaw nodes approve`). Device keypair and paired token. Setup link is "single-use and expires after 10 minutes." | SSH tunnel; Tailscale identity headers do not authenticate node connections | `exec host=node` sends `system.run` to the node. Per-node allowlists on the node host. With no target, it uses the only connected node that supports `system.run`; with several, you must pick. "A bound or explicit target that is offline... is rejected rather than redirected." | Any (gateway side) | MIT |
| Omnara `omnarad` | "Connects outbound to the control plane." No inbound ports, no SSH keys. | Org-owned machines; project grants; checked at config time and at launch. Daemon auth scheme not found in the docs I read. | Not needed (outbound) | Tools select a machine by `machine_id`. Bindings hold working directory and environment. States `online`, `asleep`, `offline`. Offline gives a retryable `machine_unreachable`. "The machine does not own the agent." Agent state is an event log in Postgres. | Any OpenAI Responses, Chat Completions, or Anthropic Messages endpoint | Apache-2.0 |
| Happy (slopus/happy) | Relay server (`happy-server`); CLI wraps Claude Code and Codex | "End-to-end encrypted mobile app". Scheme not described on the page. | Relay (not detailed) | CLI on the host runs the agent | Claude, Codex, Grok, Gemini; ACP agents as harnesses | MIT |
| Spacebot + Spacedrive | Spacedrive device library | Pairing with Spacedrive | Spacedrive | "workers can target any device in your library" | Any compatible endpoint | FSL-1.1-ALv2 |
| Hermes Agent SSH backend | SSH | SSH keys | Whatever SSH reaches | Terminal tool runs on the SSH host | Any | MIT |
| ACP | JSON-RPC over stdio for local agents; HTTP or WebSocket for remote | Not specified | Not specified | Editor to coding agent | Agent-defined | Open spec |
| Tailscale | Not an agent product. Aperture (AI gateway, GA 2026-08-26); `tsnet` gives each agent a tailnet identity. | Tailnet identity | WireGuard mesh with relays | Building block only | Gateway-level | Mixed |

Sources for the table:
- Remote Control: https://code.claude.com/docs/en/remote-control
- Codex Remote: https://learn.chatgpt.com/docs/remote-connections
- Dots: https://learn.chatgpt.com/docs/dots
- OpenClaw: https://docs.openclaw.ai/nodes , https://docs.openclaw.ai/nodes/node-exec , https://docs.openclaw.ai/nodes/node-host
- Omnara: https://github.com/omnara-ai/omnara , https://docs.omnara.com/machines/overview
- Happy: https://github.com/slopus/happy
- Spacebot: https://github.com/spacedriveapp/spacebot
- Hermes: https://github.com/NousResearch/hermes-agent
- ACP: https://agentclientprotocol.com/overview/introduction
- Tailscale: https://tailscale.com/blog/aperture-ga , https://tailscale.com/docs/develop-with-ai (via search summary; I did not open these pages)

Extra facts for the table:
- **FACT.** Remote Control: "Your local Claude Code session makes outbound HTTPS requests only and never opens inbound ports on your machine. When you start Remote Control, it registers with the Anthropic API and polls for work." Server mode allows 32 concurrent sessions by default (`--capacity`) and `--spawn worktree` gives each session a git worktree. The computer must stay on. https://code.claude.com/docs/en/remote-control
- **FACT.** Codex Remote host requirements: ChatGPT desktop app on macOS or Windows, awake, online, same account. SSH hosts are found in `~/.ssh/config`, and `codex` must be on the remote `PATH`. https://learn.chatgpt.com/docs/remote-connections
- **FACT.** Third-party blogs claim a Noise-encrypted relay and outbound-only host connection for Codex Remote. The official docs do not say this. Treat as unverified. https://codex.danielvaughan.com/2026/07/28/codex-remote-ga-noise-encrypted-relay-qr-pairing-mobile-agent-control-headless-ci-cd/
- **FACT.** ACP: "Full support for remote agents is a work in progress." https://agentclientprotocol.com/overview/introduction
- **FACT.** I found no turnkey open-source Tailscale agent mesh. Tailscale's own material is an AI gateway and identity building blocks. https://tailscale.com/blog/aperture-ga (search summary)

---

## Part 3: Lessons

### (a) Local and multi-device execution

1. **Dial out from the device.** **FACT:** Remote Control, Omnara, OpenClaw (WebSocket), and Codex (relay) all avoid inbound ports. **INFERENCE:** This solves NAT and firewall problems with no mesh VPN. Offer Tailscale or SSH as an optional second transport, not the base.
2. **Keep agent state in the hub, not on the device.** **FACT:** Omnara: "the machine does not own the agent." Claude Projects threads on a computer stop when it sleeps. **INFERENCE:** Hub-held event logs let a task survive a closed laptop, switch device, or wait for a device to wake. Your "task = sub-agent in its own thread" fits this.
3. **Support two executor types.** **INFERENCE:**
   - Tool executor: the hub runs the model loop and calls `run_command`, file tools, and MCP tools on a device (Omnara style). Works with any model.
   - Harness executor: the device runs a full harness (Claude Code, Codex, Goose, Pi) and the hub talks to it through its own protocol or ACP (Remote Control style). Gives a richer local experience but ties the model to that harness.
4. **Make device choice explicit and fail closed.** **FACT:** OpenClaw rejects an offline bound target instead of redirecting; Omnara returns a retryable `machine_unreachable`. **INFERENCE:** For "agent chooses the device," give the agent a `list_devices` tool (state, OS, tags, grants) and require a device ID on every device tool call. Never fall back silently to the cloud or to another machine.
5. **Show device state: online, asleep, offline.** **FACT:** Dots, Remote Control, and Codex all require an awake computer; Omnara models `asleep` as wakeable. **INFERENCE:** Decide per task: queue, fail, or reroute. Tell the user in the task card.
6. **Put device policy on the device, and add per-device grants.** **FACT:** OpenClaw approvals are per node host; command surface is approved separately from device identity; Omnara grants machines to projects. Dots users ask for per-device permissions. **INFERENCE:** Map this to channels: each channel has an allowed-device list and a secret list. The device also holds its own allowlist so a hub compromise cannot run any command.
7. **Pair with short-lived, single-use secrets.** **FACT:** OpenClaw setup link is single-use and expires in 10 minutes; Codex uses a QR code per phone-host pair; Remote Control uses short-lived scoped credentials and revocable trusted devices. **INFERENCE:** Use a device keypair plus revocable token. Make revoke one click.
8. **Reuse Duet's credential pattern for devices.** **FACT:** Duet puts a placeholder in the agent environment and a proxy outside it inserts the secret. **INFERENCE:** Your device daemon can do the same for API keys, so the model never reads them.
9. **LAN-only MCP servers need a device-side gateway.** **INFERENCE:** No project I read documents this. A daemon inside the LAN can list its local MCP servers as device-scoped tools and forward calls over the outbound tunnel. Treat each exposed MCP tool as a command surface that needs approval, like OpenClaw.
10. **Contain local runs with OS sandboxes.** **FACT:** Spacebot uses bubblewrap and sandbox-exec; ZeroClaw uses Landlock, Bubblewrap, Seatbelt, Docker. **INFERENCE:** None of these pages names a Windows sandbox. Plan your Windows story early.
11. **Do not wait for remote ACP.** **FACT:** ACP remote support is "a work in progress." **INFERENCE:** Use ACP only for local stdio harnesses on the device. Use your own tunnel between hub and daemon.
12. **Name the limits of the filter proxy.** **FACT:** OpenDuet admits raw sockets bypass its proxy and points to Kubernetes NetworkPolicy. **INFERENCE:** On user devices you cannot rely on a cluster policy. Prefer OS sandboxing plus an approval step.

### (b) Multi-model support

1. **Speak provider protocols, not provider names.** **FACT:** Omnara accepts any OpenAI Responses, Chat Completions, or Anthropic Messages compatible endpoint. Spacebot does the same. **INFERENCE:** Three adapters cover Anthropic, OpenAI, Fireworks, OpenRouter, Ollama, LiteLLM. Add per-provider quirks as small shims.
2. **Choose a model per task, per channel, and per call type.** **FACT:** Spacebot routes by call type and downgrades simple messages. Duet offers no choice at all. **INFERENCE:** Store the model on the task record so a report shows what ran.
3. **Handle limits with fallback chains.** **FACT:** ZeroClaw has fallback chains; Anil-matcha/open-duet falls back to another engine on a usage limit. https://github.com/zeroclaw-labs/zeroclaw , https://github.com/Anil-matcha/open-duet
4. **Keep a capability matrix.** **FACT:** In Anil-matcha/open-duet, team leads must use Claude or Codex, and image input and computers are not supported by every engine. **INFERENCE:** Model features differ (tools, vision, caching, parallel calls). Check them at routing time.
5. **Do not build on a vendor harness if you need any-model.** **FACT:** Remote Control is blocked when `ANTHROPIC_BASE_URL` points elsewhere; Projects threads use "Anthropic as the model provider in both cases." https://code.claude.com/docs/en/remote-control , https://code.claude.com/docs/en/claude-projects **INFERENCE:** Claude Code can be one optional harness executor, never the core.
6. **Treat the default model as a product choice.** **FACT:** OpenDuet defaults to an OpenAI model with Anthropic optional. https://github.com/CopilotKit/OpenDuet

### (c) Channel, task, and memory UX

1. **Keep the main chat light and delegate.** **FACT:** Spacebot channels have "no shell, no filesystem, no memory search"; Projects' coordinator "sees what threads report back, not every step they take." **INFERENCE:** The "Everything" chat should talk, route, and summarize. Tasks do the work.
2. **Tasks report back, and the user closes them.** **FACT:** Duet: "only you can mark a task as completed." OpenDuet statuses: Working, Ready for review, Needs input, Resolved. Projects: a "Waiting on you" view. Copy these.
3. **Approvals belong inside the task, with the exact action shown.** **FACT:** Projects prompts live in the thread and the coordinator cannot approve for it. Duet shows exactly what it will do. **INFERENCE:** Also surface approvals in the main chat as a link, so users do not hunt through threads.
4. **Scope memory deliberately.** **FACT:** Duet channels share one memory. Cowork and Projects scope memory per project and let you turn off account memory. **INFERENCE:** Use two layers: a global memory and a per-channel notes folder. Show which layer each fact came from. Add a per-channel "inherit global" toggle.
5. **Use files for memory.** **FACT:** Projects stores memory as files with an index (`MEMORY.md`) that users can read, edit, and delete. Anil-matcha/open-duet does the same. **INFERENCE:** Files are easy to sync to a device and to diff.
6. **One-line purpose is cheap and useful.** **FACT:** OpenDuet channels have name, colour, and purpose. **INFERENCE:** Feed the purpose into every task prompt in that channel and into the router that picks a channel for new work.
7. **Every channel gets grants.** **FACT:** Omnara shares secrets and machines with each project explicitly. **INFERENCE:** A channel holds its repos, devices, secrets, and files folder as grants, and a task inherits them.
8. **Routines need run history and breakers.** **FACT:** Anil-matcha/open-duet keeps run history, missed-run tracking, and restart recovery. Spacebot auto-disables a cron job after 3 consecutive failures. A Projects thread started by a schedule stops at a usage limit instead of waiting.
9. **Show the cost of background work.** **FACT:** Duet's background checks and brief count toward plan limits; Dots conversations do not. **INFERENCE:** Show per-task and per-routine token use so users can tune routines.
10. **Avoid a hosted dependency for core state.** **FACT:** OpenDuet cannot start without Intelligence. **INFERENCE:** This breaks your goal of local control. Own the thread store and memory store.

### Fork-base ranking (INFERENCE, from READMEs and docs only)

1. **Omnara** (Apache-2.0): best match for the device bridge and any-model goals. Not Duet-shaped. Maintenance not confirmed on the page.
2. **OpenDuet** (MIT): best match for the UX shape and AG-UI. Needs the hosted layer removed and a device bridge added.
3. **OpenClaw** (MIT): do not fork. Read the node pairing and exec-routing docs and copy the ideas.
4. **Anil-matcha/open-duet** (Apache-2.0): good reference for ACP engines, routines, approvals. Single owner, no remote machines.
5. **Spacebot** (FSL): good architecture reference. The license limits a fork for now.

Practical path: start a small hub with your own event log, copy the `omnarad` and OpenClaw node patterns for the daemon, and borrow OpenDuet's AG-UI front end and approval gateway ideas.

---

## Gaps and unverified items

- I could not read the Dots launch post or the Dots help article (HTTP 403).
- Omnara's daemon authentication scheme was not in the pages I read. Check https://docs.omnara.com/self-hosting/architecture before relying on it.
- Happy's encryption scheme and transport were not on the page. Read the repo.
- Codex Remote's Noise relay claim is from third-party blogs only.
- Whether CopilotKit Intelligence can be self-hosted is not stated in the OpenDuet docs I read.
- Goose remote execution, and Hermes use of several backends at once, were not found in the pages I read. Absence on a README is not proof of absence in code.
- The launch date of Duet differs between sources (October 21 and October 23).
- Star counts look high for OpenClaw and Hermes. They come from the page text at fetch time.
