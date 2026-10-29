# Research: turning off Claude account extras for one session

Date: 2026-10-29. Claude Code 2.1.312, claude-agent-acp 0.99.0 (pinned SDK `@anthropic-ai/claude-agent-sdk` 0.3.312).

This note supports the per-account "provider extras" toggle in `docs/CONNECTORS-SKILLS-PLAN.md`.

## What to send

Keep the profile and its user settings unchanged, so extras stay on by default. For a session with extras off, send this in `session/new` `_meta`, and send the same `_meta` again on `session/load` and resume:

```json
{"claudeCode":{"options":{
  "settings":{"disableClaudeAiConnectors":true,"syncClaudeAiSkills":false,"syncClaudeAiPlugins":false},
  "env":{"ENABLE_CLAUDEAI_MCP_SERVERS":"false"}
}}}
```

- `settings` given inline (the same as `--settings`) applies to that run only. It stops downloads and hides synced items. It moves no files.
- Do not put `syncClaudeAi*: false` in the profile's user settings. There it moves the synced folders to `.trash` at the next launch.
- `env` is a second guard for connectors. ACP builds the env as `{...process.env, ...options.env, ...providerEnv}`, so the value stays unless a provider route sets the same name.
- ACP passes `settings`, `env`, `skills`, `plugins` and `strictMcpConfig` through from `_meta.claudeCode.options`. ACP adds model config and, for a provider route, `apiKeyHelper` to `settings`; the keys above stay.
- On `session/load`, a change to `env`, `settings`, `skills`, `plugins`, `strictMcpConfig`, `settingSources` or `mcpServers` rebuilds the session.

## Do not use

- `settingSources: []`: drops all user settings, permissions and user MCP servers.
- `--bare`, `CLAUDE_CODE_SIMPLE=1`, `--safe-mode`, `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`: switch off too much.
- `strictMcpConfig`: not documented to remove claude.ai connectors, and a test showed they stayed.

## Tested and not tested

- Tested with `claude mcp list` on a signed-in profile: 19 `claude.ai …` servers by default, 0 with `ENABLE_CLAUDEAI_MCP_SERVERS=false`, 0 with `--settings '{"disableClaudeAiConnectors":true}'`.
- Not tested: that inline `settings` hides already-synced skills and plugins at run time, and that `skills: []` hides all skills. Check this first, with the `system`/`init` message.

## How to check what a session got (no credentials)

- The SDK `system`/`init` message: `mcp_servers[]` with `source` (`claudeai`, `plugin`, `user`, ...), `skills[]`, `plugins[]` with `{name, path, version?}`, and `plugin_errors[]` (synced entries are tagged `synced[N]`).
- Extras are off when no `mcp_servers` entry has `source: "claudeai"` or a name that starts with `claude.ai `, and no plugin ends with `@synced`.
- On disk, names only: `$CLAUDE_CONFIG_DIR/plugins/synced/<org>_<id>/` and `$CLAUDE_CONFIG_DIR/skills/synced/<org>_<id>/`. Never read the credential files.
- `claude mcp list` works but makes network health checks (about 60 seconds).

## Other switches

- One connector: `deniedMcpServers: [{serverName: "claude.ai Slack"}]`.
- One synced plugin: `enabledPlugins: {"<name>@synced": false}`. A plugin that the organization requires ignores this.

Sources: code.claude.com docs (`mcp`, `env-vars`, `settings-reference`, `plugins/loading`, `skills`, `cli-reference`, `headless`), the Claude Code changelog, SDK `sdk.d.ts`, and claude-agent-acp `dist/acp-agent.js` and `acp-agent.d.ts`.
