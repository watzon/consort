# Domain

A personal AI assistant that one user talks to from any client. The assistant does its work on the user's own machines with any harness and any model. The working name for the product is not chosen yet.

## People and apps

**User**:
The one person who owns the hub and talks to the assistant.
_Avoid_: owner, member, customer

**Assistant**:
The single bot identity the user talks to, with one set of skills and one memory, in every channel.
_Avoid_: agent, bot, Claude, hearthbot

**Client**:
An app that the user uses to talk to the assistant, such as the desktop app or the mobile app.
_Avoid_: frontend, device, UI

## Machines

**Hub**:
The always-on server that stores all conversations, tasks, memory and routines, and decides what runs where.
_Avoid_: server, control plane, gateway, coordinator

**Worker**:
A machine that runs sessions for the hub and gives them access to its files, repos, shell and local network.
_Avoid_: node, device, host, environment, computer, runner

**Pairing**:
The one-time step that makes the hub trust a new worker or client.
_Avoid_: registration, linking, enrollment

**Worker policy**:
The limits of a worker: the highest approval mode, the allowed folders and shell access. The user sets them in the app, and the worker keeps them and enforces them.
_Avoid_: permissions, ACL, sandbox

**Local MCP server**:
An MCP server that a worker can reach on its own network but the hub cannot.
_Avoid_: LAN server, private connector

**Tag**:
A short name on a worker, such as `work` or `gpu`, made of 1 to 32 lowercase letters, digits and `-`. The user sets user tags in the app. The worker reports detected tags, such as `macos`, `arm64`, `ram-32gb`, `cpu-12` or `gpu-apple`. A channel's tags are requirements: all work from the channel runs only on workers that have every tag of the channel.
_Avoid_: label, group, pool

**Consort home**:
The folder on a worker that holds the profile folders and the managed tools: `~/.consort` (`%USERPROFILE%\.consort` on Windows), or the folder in `CONSORT_HOME`.
_Avoid_: data folder, app folder

**Managed tool**:
A CLI or runtime, such as Claude Code or Node.js, that a worker downloads, checks and updates itself in `<consort home>/tools/<tool>/<version>/`, in place of a global install.
_Avoid_: dependency, global install, binary

## Conversation

**Tutti**:
The main channel, where the assistant can read every other channel and manage all of Consort. In music, "tutti" means that all voices play together. The UI, the code and the wire say Tutti.
_Avoid_: Everything (the old name), main chat, home, inbox, overview

**Channel**:
A conversation scoped to one subject, with its own purpose, instructions, memory, tasks, routines, files and repos.
_Avoid_: project, space, workspace, room

**Group**:
A named folder of channels in the sidebar.
_Avoid_: space, category, section

**Purpose**:
The one-line statement of what a channel is for.
_Avoid_: topic, description, goal

**Instructions**:
Longer standing guidance that the user writes for the assistant in one channel.
_Avoid_: system prompt, rules, custom instructions

**Timeline**:
The main message stream of a channel, without the replies inside threads.
_Avoid_: feed, chat log, history

**Thread**:
A chain of replies under one message in a channel.
_Avoid_: conversation, sub-chat

**Task card**:
The message in a timeline that stands for a task and shows its newest updates.
_Avoid_: task preview, thread card

## Work

**Task**:
A unit of work that the assistant or the user starts, which runs in its own session and reports in its own thread.
_Avoid_: job, subagent, ticket, todo

**Task status**:
Where a task is in its life: Up next, Scheduled, Working, Needs input, Ready for review, Failed or Resolved. Only the user moves a task to Resolved.
_Avoid_: state, stage

**Routine**:
A prompt that the hub runs on a schedule in one channel.
_Avoid_: cron job, scheduled task, automation, trigger

**Proactive turn**:
A coordinator turn with a proactive wake event, such as a check-in or a follow-up, and no event that the user caused. An update of a task that a proactive turn started is proactive too. A turn that also has a user message, an answer or a rating is a normal turn.
_Avoid_: background turn, autonomous turn

**Proactivity level**:
How much the assistant starts on its own in one channel: Off, Quiet, Normal or Eager.
_Avoid_: autonomy, mode

**Check-in**:
A routine of kind `check_in` that wakes the coordinator to look for things that need the user.
_Avoid_: heartbeat, poll

**Check-in note**:
The memory note `check-in.md` that says what to watch and what to skip. Tutti's note is in global memory. A channel can have its own note in its channel memory.
_Avoid_: heartbeat file, checklist

**Follow-up**:
A one-time wake that the assistant sets for itself, with a reason.
_Avoid_: reminder, wake-up, commitment

**Offer**:
A decision that the assistant posts on its own initiative, with the options "Do it", "Not now" and "Never about this".
_Avoid_: suggestion, proposal

**Quiet hours**:
The hours when the assistant starts nothing and sends no notifications.
_Avoid_: do not disturb, active hours

**Proactivity log**:
The list of every proactive wake and its outcome.
_Avoid_: audit log, history

**Coordinator**:
The session that speaks for the assistant in one channel.
_Avoid_: main agent, orchestrator, channel bot

**Session**:
One conversation with a harness on one worker, for a task or for a coordinator.
_Avoid_: run, agent, process, thread

**Turn**:
One input to a session and all the work the session does before it waits again.
_Avoid_: request, step, exchange

**Placement**:
The choice of worker and workspace for a session.
_Avoid_: scheduling, routing, assignment

**Workspace**:
The folder on a worker where a session runs.
_Avoid_: sandbox, environment, working directory

**Repo**:
A git repository that is attached to a channel.
_Avoid_: project, source, codebase

**Checkout**:
A copy of a repo on one worker that the hub knows about.
_Avoid_: clone, local copy, mirror

**Isolation**:
How a task's workspace relates to a checkout: in place, a new worktree, or a fresh clone.
_Avoid_: sandboxing, mode

## Harnesses and models

**Harness**:
An agent program that runs a model in a loop with tools, such as Claude Code, Codex, Pi or OpenCode. Code uses this word.
_Avoid_: engine, runtime, CLI

**Provider**:
The UI word for a harness that Consort can install and run: Claude Code, Codex, OpenCode, Gemini CLI, an agent from the ACP registry, or a custom ACP command. The UI shows each provider with its logo.
_Avoid_: provider for a model company (say model provider), vendor, engine

**Session host**:
The process on the hub machine that holds the harness CLIs, the accounts and the sign-ins, and runs each session in a boundary. It is not the hub: it has its own credentials and, on Linux, its own user. The UI says "the hub machine".
_Avoid_: hub worker, local worker

**Execution worker**:
A machine where work runs: commands, files and checkouts. It has no account and no CLI. The UI and most of the code say **worker**.
_Avoid_: runner, agent machine

**Boundary**:
The OS limits around a session: bubblewrap on Linux, `sandbox-exec` on a Mac in development, and a network proxy for each session. With no boundary, no session starts.
_Avoid_: jail, sandbox (in code; the UI may say "development sandbox" for a Mac)

**Command check**:
In Auto mode, the model that decides whether a shell command or an unknown tool can run without asking the user. It allows, blocks or asks. The code calls it the classifier or `command_check`. It is not a lock: worker policy and the boundary still hold.
_Avoid_: classifier (in UI text), guard, safety check

**API endpoint**:
A base URL and a key that the user adds in Settings > Providers. Only the command check uses it. It cannot run a task.
_Avoid_: provider, account

**Journal**:
The worker's record of each tool operation, by session, operation ID and argument hash. A retry with the same key gets the stored result and does not run again.
_Avoid_: log, history

**Harness account**:
One login to one harness on the hub machine, with its own profile folder, such as "Codex Personal". The UI calls it an **account**. A worker never holds one.
_Avoid_: provider instance, instance, profile

**Profile folder**:
The folder of one account in the session host, `<consort home>/profiles/<name>/`, where the name comes from the account name when the account is made (`Claude Work` gives `claude-work`) and does not change on a rename. It holds the harness's own config and login, and `env.json` with the account's secret variables.
_Avoid_: home, shadow home (for a new account), config dir

**Sign-in**:
One run of a provider's sign-in flow for one account, which the user starts from the app.
_Avoid_: login (for the flow), auth, OAuth

**Model provider**:
The company that serves a model, such as Anthropic, OpenAI or Fireworks.
_Avoid_: provider (alone, which means a harness), vendor, backend

**Model selection**:
A harness account label, a model and its options, such as "Claude Personal, Opus 5.5, high effort".
_Avoid_: profile, preset, model config

**Favorite model**:
An account label and a model that the user marked with a star in the model picker. The hub keeps the favorites in the user's preferences, so every client shows the same ones.
_Avoid_: pinned model, starred model

**Approval mode**:
How much a session may do without asking: Ask, Auto edits, Auto or Full access.
_Avoid_: runtime mode, permission mode, autonomy

## Waiting on the user

**Approval**:
A harness request for permission to do one action, which the user allows or denies.
_Avoid_: permission request, prompt, confirmation

**Decision**:
A question from the assistant with two to four labelled options, each with its consequence.
_Avoid_: question card, poll, choice

**Needs input**:
The task status while an approval or a decision waits for the user.
_Avoid_: blocked, waiting, paused

**Card**:
A message in the timeline that waits for the user, which the hub makes itself and shows like an approval: a connect card, a sign-in card, an install card, a management approval card, or a connector approval. Code calls it a hub card.
_Avoid_: prompt, popup, form, modal

## Knowledge

**Memory**:
The notes that the assistant keeps across conversations.
_Avoid_: knowledge base, context, facts

**Memory note**:
One markdown file in memory, addressed by its path.
_Avoid_: memory entry, fact, record

**Global memory**:
The memory about the user that every channel reads and only Tutti writes.
_Avoid_: user memory, shared memory, profile

**Channel memory**:
The memory that belongs to one channel, which Tutti can also read.
_Avoid_: project memory, local memory

**Voice**:
The user's own way of writing to other people, which the assistant can use for drafts that go out in the user's name when "Write in my voice" is on. It changes only the wording, never a fact, a date, a price or an agreement.
_Avoid_: persona, tone of the assistant, writing style (for the assistant's own replies)

**Style guide**:
The global memory note `voice.md` that describes the voice: Core rules, one section for each context (Formal email, Close email, Chat, Public posts), short examples with placeholders, and hard rules. A rule that ends with `(you)` is one that the user wrote, and a learned change never replaces it. The note changes when the user edits it, asks Tutti in a message to edit it, or says Do it on an offer.
_Avoid_: voice profile, persona file, style model

**Dictation**:
Speech that becomes text in a message box while the user speaks. It runs on the user's computer (the client), not on the hub or on a worker. The heard words are gray until dictation ends.
_Avoid_: voice (that is the user's way of writing), transcription, system dictation

**Commitment flag**:
A phrase in a draft that the user must check before it goes out: a date, a time, a weekday, a price or an agreement whose value the request (the user's own words) did not contain, or text that breaks a hard rule.
_Avoid_: warning, risk marker, highlight

**Draft pair**:
A draft that the assistant wrote and the text that went out, which the hub keeps for 90 days. A pair that the user sent unchanged is the assistant's text and is never used for learning.
_Avoid_: edit history, diff

**Draft card**:
A card with text that goes out in the user's name. A connector card of an outgoing tool shows a title (the subject, or "Email to <name>"), the To and connection lines, the text and the words Send and Not now (Save draft for a tool that only saves a draft). After the answer it says "Sent to <name>", "Not sent" or "Replaced by a new version", and the text that went out. A message of the user that arrives while the card waits closes it as "Replaced by a new version" and goes to the assistant, which writes the draft again or answers. A card from `show_draft` is for text that no connection can send. It waits for nobody, and the user edits it or copies it.
_Avoid_: preview, send dialog

**Task line**:
The one line that stands for a task in the chat: the status glyph, the title and the state ("Working, 2 min", "Needs you", "Ready for review"). While the task runs, it has no card under it. A click opens the task thread. Resolve is in the thread.
_Avoid_: task card (the card is only for a pending approval, a failed task or the voice task)

**Plan**:
The checklist that the harness makes for a task. The thread shows it as a card under the newest message, and its header says "Working, <current step>".
_Avoid_: todo list, steps

**Memory suggestion**:
A note that a task or a channel coordinator proposes for memory, which the coordinator one level up accepts or rejects.
_Avoid_: memory proposal, pending memory

**Channel files**:
The shared folder of files that belongs to one channel and its tasks.
_Avoid_: library, attachments, drive

**Artifact**:
A file that a task publishes as a result of its work.
_Avoid_: output, deliverable, attachment

**Hub tools**:
The tools that the hub gives every session so it can post messages, start tasks, ask decisions and use memory.
_Avoid_: MCP tools, assistant tools, built-ins

**Management tool**:
A hub tool with which the assistant changes channels, workers, accounts, connections or skills. The Tutti coordinator manages everything, and a channel's coordinator manages only its own channel. A risky change waits for the user on a management approval card.
_Avoid_: admin tool, settings tool

## Connectors and skills

**Connector**:
A service from the connector catalog, such as Gmail or GitHub, that the assistant can use through Composio. The UI calls it a connector, and Composio calls it a toolkit.
_Avoid_: integration, app, toolkit (in the UI), MCP server

**Connection**:
One connected account of one connector, such as "Gmail Work", with a name, a handle that the assistant uses (`gmail_work`), a "Use in" scope and an approval rule for each tool kind.
_Avoid_: connected account, link, credential

**Tool kind**:
What a connector tool does: read, change (create or update) or delete. It picks the approval rule: run without asking, ask once per task, or ask every time.
_Avoid_: permission level, scope

**Monid**:
A catalog of paid tools with one wallet. The hub uses it with a key that only the hub holds. Monid has no accounts to connect, so it is not a connector.
_Avoid_: marketplace, Monad

**Paid tool**:
A tool from Monid. Each run can cost money.
_Avoid_: service, endpoint (in the UI), connector tool

**Monid call**:
One run of a paid tool, with a row in the chat or the task thread that shows the tool, the input, the output and the cost.
_Avoid_: run (alone), purchase

**Skill**:
A folder in the Agent Skills format, with a `SKILL.md`, that the hub installs from a source, pins to a commit and a content hash, and gives to sessions.
_Avoid_: plugin, prompt, recipe, tool

**Use in**:
Where a connection or a skill can be used: Tutti only, every channel, the channels that have all of some tags, or a list of channels. Tutti only is a list with only Tutti. Tutti matches tags only when it has tags of its own.
_Avoid_: scope (in the UI), visibility, sharing

**Channel exception**:
A connection or a skill that one channel turned off, though its "Use in" gives it to the channel. A channel cannot turn on an item that its "Use in" does not give it.
_Avoid_: override, block list

**Effective set**:
The connections and skills that a session gets: the ones whose "Use in" matches the session's channel, minus the channel's exceptions.
_Avoid_: enabled tools, active connectors

### Access request

A session asks the user for a connection or for Monid that its channel lacks. The user answers on a card with temporary access, lasting access or no. Access does not change approvals.

### Temporary access

Access that ends by itself: with its task, or after 24 hours in the chat. It changes no "Use in", and the user can end it in the Channel panel.
