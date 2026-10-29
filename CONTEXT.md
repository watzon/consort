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
The limits that a worker sets on what the hub may ask it to do.
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

**Everything**:
The main channel, where the assistant can read every other channel.
_Avoid_: main chat, home, inbox, overview

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

**Harness account**:
One login to one harness on one worker, with its own profile folder, such as "Codex Personal" on the MacBook Pro. The UI calls it an **account**. Accounts with the same name on different workers count as one account for a model selection.
_Avoid_: provider instance, instance, profile

**Profile folder**:
The folder of one account on its worker, `<consort home>/profiles/<name>/`, where the name comes from the account name when the account is made (`Claude Work` gives `claude-work`) and does not change on a rename. It holds the harness's own config and login, and `env.json` with the account's secret variables.
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

## Knowledge

**Memory**:
The notes that the assistant keeps across conversations.
_Avoid_: knowledge base, context, facts

**Memory note**:
One markdown file in memory, addressed by its path.
_Avoid_: memory entry, fact, record

**Global memory**:
The memory about the user that every channel reads and only Everything writes.
_Avoid_: user memory, shared memory, profile

**Channel memory**:
The memory that belongs to one channel, which Everything can also read.
_Avoid_: project memory, local memory

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
