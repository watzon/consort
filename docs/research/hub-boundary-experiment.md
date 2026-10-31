# Research: can a CLI on the hub do all its work through Consort tools?

Date: 2026-10-30. Host: this Mac (Darwin 27.0.0, arm64). Claude Code 2.1.314, set with `CLAUDE_CODE_EXECUTABLE`. `claude-agent-acp` 0.99.0, SDK 0.3.312.

This note is the result of step 1 of "Order" in `docs/HUB-PLAN.md`.

## Verdict

The session host split can go ahead. The direction does not change. The plan needs the changes in "Changes to the plan".

Step 1 is not complete as the plan wrote it. These parts did not run:

- A Windows worker.
- A real model on a Linux session host. Linux ran with a fake provider, because a new Linux profile needs the owner to sign in.
- A real second machine. The worker ran on the same Mac as the host.
- A script skill.

Finish these before step 4 (tasks move). They do not block step 2.

| # | Condition | Result |
|---|---|---|
| 1 | Edit-and-test task through Consort tools only | Passed on the Mac with Haiku |
| 2 | Native tools unavailable | Passed, only with the generated config. The config is not kept across a session load |
| 3 | No access to another profile or hub data | Files passed on macOS and Linux. On macOS, keychain items are not separated per profile |
| 4 | Network | Passed, with a new egress proxy that the plan does not have |
| 5 | Resume in a stable folder | Passed |
| 6 | A dropped reply repeats no change | No repeat in any run, but the journal gave a false success. The journal design must change |
| 7 | Restart during a turn | Recorded. A session host restart ends the turn and the worker's command finishes. A hub restart does not stop the turn, with a stand-in hub |
| | One-shot call in the boundary | Median 2.2 s, maximum 3.4 s, n=6. This is inside the 10 s timeout |

## How it was tested

The prototype and the logs are in the worktree `t3-a3858142`, at `.scratch/hub-boundary/` and `.scratch/hub-boundary/evidence/`. They are not committed.

- `worker.mjs`: the stand-in execution worker. It has `workspace.inspect`, `fs.read`, `fs.apply_patch` (hash-checked), `process.start`, `process.read` and `process.wait`, and a journal on disk.
- `mcp-endpoint.mjs`: the MCP endpoint of the session host, with a bearer token. It forwards to the worker.
- `acp.mjs`, `common.mjs`: the ACP client. It sends the same `initialize` capabilities as `consort-harness`.
- `boundary.mjs`: writes the `sandbox-exec` profile. It denies by default and allows by list.
- `proxy.mjs`: an HTTP CONNECT proxy with a host allow list.
- `mock-provider.mjs`: a fake Anthropic API. It records what each request offers the model, and it can send a `tool_use` for a tool that was not offered. Most of condition 2 used this, so it used no model.

Real model runs used the signed-in profile as it is, with `--model haiku` and low effort. Credentials were not read, printed or copied. The CLI got its own login through its own call to `/usr/bin/security`, inside the boundary.

Each test ran like this, from `.scratch/hub-boundary/`, with a clean environment:

```
node worker.mjs <port> <repo> <journal> &
env -i USER_REAL=$USER HOME_REAL=$HOME REAL_PROFILE=$CLAUDE_CONFIG_DIR PATH=/usr/bin:/bin:/usr/sbin:/sbin \
  node <test>.mjs <port> <repo>
```

Tests: `t1-surface`, `t2-forced`, `t3-battery`, `t4-cli-in-boundary`, `t5-oneshot`, `t6-real`, `t7-drop`, `t7b-restart`, `t8-pending`, `t10-misc`, `t11-process`. Linux: `linux/test.mjs`, `linux/test2.mjs`.

Some early runs were wrong because of faults in my tests, and I corrected and reran them. The fake provider lost its step count, and a blocking `spawnSync` stopped the proxy in the same process. Only the corrected results are here. Logs of the bad runs are also in `evidence/`.

## 1. Edit-and-test task: passed

The scratch repo had `add` with `-` in place of `+`. The prompt was to run `sh test.sh`, fix the bug, and run the test again. Worker log (`evidence/worker-real.log`):

```
RUN workspace.inspect - {}
ERR process.start path must be relative to the capability: /private/tmp/hub-exp/sessions/real1
RUN process.start run-tests-2 {"command":"sh test.sh"}
RUN process.wait - ...
RUN process.start inspect-files-1 {"command":"ls -la && cat test.sh && ..."}
RUN process.wait - ...
RUN fs.read - {"path":"calc.sh"}
RUN fs.apply_patch fix-add-operator-1 {"path":"calc.sh","expected_sha256":"6da76d1d..."}
RUN process.start run-tests-3 {"command":"sh test.sh"}
RUN process.wait - ...
```

Result on the worker: `git diff` shows only the `-` to `+` change, and `sh test.sh` prints `PASS`. The hub-side session folder stayed empty. There were no permission prompts.

The one error is a finding. The model gave the hub-side folder as `cwd`, because the CLI names that folder in its prompt. See "The wrong environment".

## 2. Native tools: passed, with conditions

### What the model is offered

From the `tools` field of the provider request, with a planted profile and a planted session folder (`evidence/t1.out`):

| Config | Non-Consort tools offered | Planted items that ran |
|---|---|---|
| No restrictions (Full access today) | `Agent`, `Bash`, `Edit`, `Read`, `Write`, `Skill`, `ToolSearch`, `Workflow` and more | Hooks, `apiKeyHelper`, 3 MCP servers |
| `settingSources: ["user"]` (below Full access today) | The same | User hooks, user `apiKeyHelper`, user MCP server |
| `tools: []` only | None | Hooks, `apiKeyHelper`, 3 MCP servers |
| Generated config, each mode (`default`, `acceptEdits`, `auto`, `bypassPermissions`) | None | None |

The plan's name `workspace.inspect` reaches the model as `mcp__consort__workspace_inspect`. The call still reaches the dotted name. The list also holds `DeferredToolPlaceholder`, with the description "never call this tool". It does nothing that I could find.

`tools: []` alone is not enough. It removes tools from the model, but commands from settings still run on the host.

### Which setting does what

One setting removed from the generated config at a time (`evidence/t2b.out`):

| Removed | Effect |
|---|---|
| `tools: []` | All native tools return |
| `settingSources: []` | `CLAUDE.md` and rules reach the prompt, and project `apiKeyHelper` runs, even with hooks off |
| `settingSources: ["user"]` in place of `[]` | User `apiKeyHelper`, `CLAUDE.md` and rules return. This is today's value below Full access |
| `strictMcpConfig`, `disableAllHooks`, `plugins`, `agents` | No change in this test. `settingSources: []` already covers them. Keep them as a second guard |

### Forced calls

The fake provider sent `tool_use` for tools that were not offered, in Ask and in Full access (`evidence/t2c.out`). All were refused. No sentinel file appeared, `hub.db` did not change, and no secret reached the provider. There were no permission prompts.

```
Bash: ERR No such tool available: Bash. Bash is disabled for this session, in subagents as well as here.
PowerShell, Monitor, Read, Write, Edit, NotebookEdit, Glob, Grep, WebFetch, WebSearch, Agent, Task, Skill,
ToolSearch, ListMcpResourcesTool, ReadMcpResourceTool, EnterWorktree, CronCreate, RemoteTrigger, Workflow,
TodoWrite, ExitPlanMode, AskUserQuestion, mcp__eviluser__x, mcp__evilproj__x: ERR No such tool available
mcp__consort__workspace_inspect: OK
```

### Other ways out

In `evidence/t2.out`, prompts `!touch ...`, `/evilcmd`, `/evilpc`, `/evilskill`, `/evilps`, `/init`, `/doctor`, `/mcp`, `/hooks`, `/bashes` and `/add-dir` ran nothing. Planted skills, commands and agents did not load.

### The real model

Asked to use Bash, Read, Edit, Write, WebFetch, Agent and Skill by name, Haiku replied that none is available. It made no native call, in the Ask config and in Full access (`bypassPermissions` with the same config).

### The process

During a real turn in the boundary (`evidence/t11.out`):

```
child process names: [ '.../versions/2.1.314', 'security', '(security)', '(2.1.314)' ]
established TCP peers of the CLI: [ '127.0.0.1:57520', '127.0.0.1:57519' ]   (the MCP endpoint and the proxy)
```

The only child is `security`, which the CLI uses for its login. The sample polled `ps` each 150 ms, so a shorter child could be missed.

### The generated config must be sent on each load

`session/load` with no config brought back all native tools: `Agent`, `Bash`, `Edit`, `Read`, `WebFetch`, `Write` and more. The session keeps no restriction. This fails open. So one function must build this config, and no path may start or load a session without it.

## 3. File boundary: passed, with one macOS exception

### macOS `sandbox-exec`

`evidence/t3.out`. Denied with `Operation not permitted`: read and write of `hub.db`, read and list of blobs, read and write of another profile, read through a symlink, a hard link to `hub.db`, copy, rename of the hub folder, listing of the home folder, and write to `/tmp` outside the session. A nested `sandbox-exec` with `(allow default)` was refused too.

Allowed: write to the session folder and to its temporary folder.

Two weak points on macOS:

- `file-read-metadata` is allowed, so `stat hub.db` shows that the file exists and its size. The CLI needs metadata for path lookups. I did not try to narrow it.
- `(allow process-exec)` is global.

### Keychain

A sandboxed CLI with no keychain rule returns `Not logged in`. With the keychain rule it signs in, and no credentials were copied. So the sandboxed profile can authenticate.

The rule opens the keychain daemon, and the sandbox cannot limit it to one item:

```
keychain rule off: security: SecKeychainSearchCreateFromAttributes: One or more parameters ... ; list-keychains rc=1
keychain rule on:  security: SecKeychainSearchCopyNext: The specified item could not be found ; list-keychains rc=0
```

The search is real with the rule on. So on the Mac, a process in the boundary can ask for the item of another profile. I did not try this on a real item, by the rule of this task, and I did not test whether macOS would ask for permission. Treat it as open. It needs a process in the boundary, and with native tools off the only such process is the CLI itself.

On Linux the login is a file in the profile, so a bind mount of one profile separates it. That is not tested with a real login.

### The CLI's own subprocesses, with the first wall removed on purpose

`evidence/t4.out`. Native tools on, `bypassPermissions`, user settings on, inside the boundary. A `UserPromptSubmit` hook, a stdio MCP server, `Bash` and a background job each tried to read `hub.db`:

```
hook-stole.txt "cat: /private/tmp/hub-exp/standin/hub.db: Operation not permitted\n"
hook-net-ok (absent)
mcp-stole.txt  "cat: ...: Operation not permitted\n"
bg-stole.txt   "cat: ...: Operation not permitted\n"
hub HTTP hits from inside: []
```

Other results in the boundary: `kill -0` of an outside process, `ps`, `open -a`, `launchctl`, `pbpaste`, and AppleEvents with `osascript` were all denied.

### Linux bubblewrap, in Docker

`evidence/linux.out`, with the bundled CLI 2.1.312 and a fake provider. `hub.db`, blobs and the other profile do not exist in the boundary (`No such file or directory`). With native tools on, hooks, a stdio MCP server and `Bash` got the same result. `--unshare-pid` hides outside processes.

Default Docker refuses bubblewrap:

```
bwrap: No permissions to create new namespace, likely because the kernel does not allow non-privileged user namespaces.
```

It worked only with all three of these: `--security-opt seccomp=unconfined`, `--security-opt apparmor=unconfined`, `--security-opt systempaths=unconfined`. That weakens the container. I did not try a narrower seccomp profile, or bubblewrap under systemd.

### Landlock

Kernel 7.2 reports ABI 10, and the calls work in default Docker. With my rule set, file rules and TCP port rules hold, and the CLI ran under it with the generated config. But these stay open (`evidence/linux3.out`, `linux.out`):

```
pathname unix socket -> connected, read: HUBSOCK
ALLOWED  udp to 1.1.1.1:53
ALLOWED  resolve name
ALLOWED  ps sees hub process
```

My rule set handles only the older access rights, so a newer right may close the unix socket. I did not test that. Use Landlock as a second layer, not in place of bubblewrap.

## 4. Network: passed, with an egress proxy

| From inside the boundary | macOS `sandbox-exec` | Linux bubblewrap |
|---|---|---|
| Stand-in hub on loopback | Denied | Denied (own network namespace) |
| Other loopback ports | Denied | Denied |
| MCP endpoint | Allowed by port | Reached through a unix socket bridge |
| Proxy | Allowed by port | Reached through a unix socket bridge |
| `1.1.1.1:443`, the Anthropic IP, UDP | Denied | Denied |
| DNS | Denied | Denied |
| Docker socket | Denied | Not in the boundary |
| Listen on loopback | Denied | Allowed, private to the namespace |

The proxy, on both systems:

```
CONNECT 127.0.0.1:<hub> -> 403 Forbidden
CONNECT localhost:<hub> -> 403 Forbidden
CONNECT example.com:443 -> 403 Forbidden
CONNECT api.anthropic.com:443 -> 200 OK
```

`sandbox-exec` cannot allow one provider by itself:

```
sandbox-exec: host must be * or localhost in network address
(allow network-outbound (remote tcp "*:443")) -> 1.1.1.1:443 succeeded
```

So a port rule either opens the whole internet on that port or nothing. The answer on both systems is to deny the network and give the CLI one proxy with a host allow list. With real runs on the Mac the proxy saw only `api.anthropic.com:443`. The CLI obeys `HTTPS_PROXY`, and `NO_PROXY` must name `127.0.0.1`, so that MCP goes direct.

On Linux, an invalid key through the proxy returned `Invalid API key` from the real provider, so the path works.

Not tested: a token refresh. All runs had a valid token and a host list of one, and `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` was set. A refresh may need another host. Find it before this ships, or sessions will fail when a token ends.

The bearer token of the MCP endpoint is on the CLI's command line:

```
--mcp-config {"mcpServers":{"consort":{"type":"http","url":"...","headers":{"Authorization":"Bearer <64-HEX-TOKEN>"}}}}
```

Any process of the same user outside a boundary can read it with `ps`. Inside `sandbox-exec` and bubblewrap, `ps` does not show it. Under Landlock alone it does. The token must stay scoped to one session.

## 5. Resume: passed

After the process ended, a new process used `session/load` with the same session ID and folder, a new MCP port and a new token. `loadSession` is true in the capabilities. The stronger proof is in condition 7: the resumed Haiku session called `process.read` with a process ID that only its transcript held.

The transcript is in `<profile>/projects/<encoded cwd>/`. So in the boundary the profile needs write access only there, and the real runs worked with the rest of the profile read-only.

The load also found its history from a different folder (fake provider, `evidence/t10.out`). So a stable folder is good practice, and this version does not enforce it. I would not rely on that.

## 6. Dropped replies: no repeat, but the design must change

Every case used the real model. "Lines" counts the runs of `echo run >> file`.

| Fault | Who picks the ID | Runs | Result |
|---|---|---|---|
| Worker link reply lost, host retries, `process.start` | Model | 1 | 1 line, journal replay |
| CLI reply lost, `process.start` | Model | 3 | 1 line each. The model retried with the same ID each time, and the journal replayed |
| CLI reply lost, `process.start` | Host | 3 | 1 line each. The model did not retry. It read the file or asked |
| CLI reply lost, `fs.apply_patch` | Model | 1 | Applied once. Same ID retried, journal replayed |
| CLI reply lost, `fs.apply_patch` | Host | 1 | Applied once. The model read the file and did not retry |

The CLI does not retry a tool call by itself. It gives the model `ECONNRESET`. The sample is small and used Haiku only. With host-picked IDs, safety depends on the model's judgment.

### The defect

A model picks IDs that repeat, such as `note-alpha-beta-1` and `run-test-sh-1`. A journal keyed on the ID alone returned two false results:

- After I reset `note.txt`, a new patch with the same ID replayed `ok`. The file stayed `alpha`, and the model said `Done.`
- In a later session with the same journal file, `run-test-sh-1` replayed a process from an earlier worker run. The process record was gone, so `process.wait` failed with `unknown process`, and the test never ran (`evidence/worker-real4.log`).

### The design that fixes the first one

`worker2.mjs` keys on session, ID and a hash of the arguments, and checks the file on a replay of a patch. This is a unit test, not a run with a model:

```
first                   {"ok":true,...}                              alpha-beta
retry                   {"ok":true,...,"replayed_from_journal":true} alpha-beta
after a reset, same ID  {"ok":false,"error":"state_changed_since_operation",...} alpha
other session, same ID  {"ok":true,...}                              alpha-beta
same ID, other args     {"ok":false,"error":"operation_id_reused",...}
```

The CLI sends `_meta: {"claudecode/toolUseId": "toolu_..."}` on each `tools/call`. The host can use it as the ID on the worker link. A retry by the model has a new tool use ID, so the model-picked ID is still needed for that case.

## 7. Restart during a turn: recorded

### Session host killed with SIGKILL, real model

The turn was in `process.wait` on `sleep 40; echo finished >> late.log` (`evidence/t7b.out`).

- At 1.5 s the adapter is gone, because it exits on stdin EOF. The CLI has parent PID 1.
- At 9.5 s the CLI is gone. No orphans. The turn is lost.
- The worker's command goes on. `late.log` has `finished`.
- The transcript ends on the `process_wait` tool use, with no result.
- A new session host, with a new port and token, loads the session. Haiku called `process.read` and `fs.read` and reported the result. `late.log` has 1 line.

The prompt after the restart told the model not to start the command again. So this does not show what a model does when it is not told. The plan's "checks the open operations" remains necessary. A restart of the worker itself was not tested, but the `unknown process` case above shows that process records must survive it.

### Hub stand-in down for 25 s, session host up, fake provider

The tool call waited at the endpoint, and the turn finished with no error. The session host's approval call simply retried. This shows that the CLI tolerates the wait. It does not test the real hub.

### The limit on a pending tool call

This is new, and it matters for approvals and for long commands. With the approval held at the MCP endpoint, on 2.1.314:

| Reply type | Setting | Held for | Result |
|---|---|---|---|
| JSON | None | 700 s | Failed at 60 s: `The operation timed out` |
| JSON | `MCP_TOOL_TIMEOUT=600000` | 90 s | Worked |
| SSE, silent | None | 400 s | Failed at 300 s, on the tool idle timeout |
| SSE, comment each 15 s | None | 700 s | Failed at 300 s |
| SSE, `notifications/progress` each 15 s | None | 700 s | Worked |
| SSE, silent | `CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT=0` | 330 s | Worked |
| `session/request_permission`, no `allowedTools` | None | 330 s | Worked |

A wait of hours was not tested. A timeout has the same effect as a dropped reply: the work may have run, and the model does not know.

`consort-worker/src/mcp.rs` uses `StreamableHttpServerConfig::default()`. If its keep-alive is an SSE comment, it does not reset this idle timeout. Check that when this is built.

## One-shot call for the command check

Command (profile read-only, `evidence/t5.out`):

```
claude -p 'Command: git status --short. Answer now.' --model haiku --effort low --tools '' --setting-sources '' \
  --strict-mcp-config --no-session-persistence --system-prompt '<policy>' --output-format json --max-turns 1
```

| Setup | n | Min | Median | Max |
|---|---|---|---|---|
| No boundary | 6 | 741 ms | 1107 ms | 1404 ms |
| Boundary, keychain, proxy | 6 | 1348 ms | 2166 ms | 3353 ms |
| Boundary, keychain, no `HTTPS_PROXY` | 1 | Killed by the test at 45 s, no answer | | |

The answer was `allow` in each successful run, with `claude-haiku-5-5`. Each call used a little of the plan's limits. A small sample at one time of day says nothing about load.

The classifier's own deadline is necessary, because a network fault hangs for over 45 s and gives no error. The plan's rule that a timeout asks the user covers this. The classifier uses the same boundary and proxy, and needs no MCP endpoint.

## The wrong environment

The CLI adds a system message on each request:

```
# Environment
 - Primary working directory: /private/tmp/hub-exp/t9/session
 - Is a git repository: false
 - Platform: darwin
 - Shell: zsh
 - OS Version: Darwin 27.0.0
```

It stays with a custom `systemPrompt` string, with `excludeDynamicSections: true`, and with `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1`. I found no way to remove it. On a Linux hub with a Windows worker, the model is told Linux. In condition 1 this already cost one failed call. The worker's refusal of the absolute path caught it.

## What the boundary needs: findings from failures

- Without read access to `/private/var/db/timezone`, the CLI uses 100 percent CPU forever and prints nothing. `--version` still works, so a version probe does not find this.
- Such a hung CLI ignores the loss of its parent. Two ran for 23 and 28 minutes under PID 1 at 98 percent CPU until I killed them.
- Without `CLAUDE_CODE_TMPDIR`, the CLI exits with `EPERM ... open '/tmp/claude-501'`. That folder is shared by all sessions of a user, so give each session its own.
- Without `USER`, the CLI reports `Not logged in`. On macOS, `HOME` must also be the real home, because `security` finds the login keychain from it. The exact cause of that second need is my inference: all real runs used the real `HOME`.
- The adapter copies its own `process.env` into the CLI. So the session host must start it with a clean environment.
- The adapter bundles CLI 2.1.312. A pin to 2.1.314 holds only with `CLAUDE_CODE_EXECUTABLE`.
- `sandbox-exec` worked on Darwin 27.0.0, but its man page says that the command is deprecated. A macOS update could remove it. This is for development only, as the owner decided.

## The config that a session host must generate

Send this in `_meta.claudeCode.options` on every `session/new` and every `session/load`, in every mode:

```json
{
  "tools": [],
  "settingSources": [],
  "strictMcpConfig": true,
  "allowedTools": ["mcp__consort"],
  "plugins": [],
  "agents": {},
  "promptSuggestions": false,
  "allowDangerouslySkipPermissions": false,
  "settings": {
    "disableAllHooks": true,
    "disableClaudeAiConnectors": true,
    "syncClaudeAiSkills": false,
    "syncClaudeAiPlugins": false,
    "autoMemoryEnabled": false
  },
  "env": {
    "ENABLE_CLAUDEAI_MCP_SERVERS": "false",
    "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1"
  }
}
```

Give `mcpServers` in the ACP request, with only the `consort` server.

The test config also had `includeCoAuthoredBy: false` and `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1`. I did not test without them. They should have no effect with no tools.

Set the session mode to `default`. Do not use `bypassPermissions`, even for Full access. The CLI has only Consort tools, `allowedTools` stops it from asking, and the approval engine decides at the endpoint. I ran Full access both ways, and the tool surface is the same. `default` keeps the bypass flag off.

Start the adapter with exactly this environment:

```
PATH=<fixed>  LANG=en_US.UTF-8  USER=<user>  HOME=<real on macOS, session folder on Linux>
TMPDIR=<session tmp>  CLAUDE_CODE_TMPDIR=<session tmp>
CLAUDE_CONFIG_DIR=<profile>  CLAUDE_CODE_EXECUTABLE=<pinned binary>
DISABLE_AUTOUPDATER=1  CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1
HTTPS_PROXY=http://127.0.0.1:<proxy>  HTTP_PROXY=http://127.0.0.1:<proxy>  NO_PROXY=127.0.0.1,localhost
```

macOS boundary, wrapping the adapter, so that all children inherit it. The full file is `evidence/real1.sb`.

```
(version 1) (deny default)
(allow process-fork) (allow process-exec) (allow signal (target same-sandbox))
(allow process-info* (target same-sandbox)) (allow sysctl-read) (allow file-read-metadata)
(allow file-read* /usr /bin /sbin /System /private/var/db/dyld /private/var/db/timezone /private/var/select /private/etc <node> <adapter> <cli>)
(allow file-read* file-write* <session folder> <session tmp>)
(allow file-read* <profile>) (allow file-write* <profile>/projects/<encoded cwd>)
(allow network-outbound (remote tcp "localhost:<mcp>") (remote tcp "localhost:<proxy>"))
(allow mach-lookup <logging, notify, trustd, configd, opendirectory>)
(allow mach-lookup com.apple.SecurityServer com.apple.securityd.xpc ...)   ; the login
(allow file-read* file-write* ~/Library/Keychains)                         ; the login
```

Linux boundary:

```
bwrap --unshare-all --die-with-parent --new-session --clearenv --setenv ...
  --ro-bind /usr /usr --symlink usr/bin /bin --symlink usr/lib /lib --symlink usr/sbin /sbin
  --ro-bind /etc/ssl /etc/ssl --ro-bind /etc/passwd /etc/passwd --ro-bind <adapter> <adapter>
  --proc /proc --dev /dev --tmpfs /tmp
  --bind <profile> <profile> --bind <session> <session> --bind <session tmp> <session tmp>
  --bind <socket folder> /run/consort --chdir <session>
  sh -c 'socat TCP-LISTEN:<mcp>,bind=127.0.0.1,fork UNIX-CONNECT:/run/consort/mcp.sock &
         socat TCP-LISTEN:<proxy>,bind=127.0.0.1,fork UNIX-CONNECT:/run/consort/proxy.sock &
         sleep 0.2; exec node <adapter>'
```

## Changes to the plan

1. **Add an egress proxy to the session host.** "Rules" says that a CLI must not reach loopback services, but not how it reaches its provider. Deny the network in the boundary, and give each CLI one proxy with a host list for its provider. This is the only design that worked on both systems.
2. **Change "Full access too" to one fixed config.** Use the same options and the `default` mode in all four modes. Only the approval engine differs. Today's `settingSources: ["user"]` must not reach the hub, because user `apiKeyHelper`, `CLAUDE.md` and rules still load.
3. **One function builds the config, for new sessions and for loads.** A load without it restores all native tools. Add a contract test on the fake provider, run for each version bump: planted profile, exact tool list, no sentinel. Enforcement is proven for this one version only.
4. **Say how approvals wait.** Hold them at the endpoint, on SSE, with `notifications/progress` each 15 s. Keep `process.wait` short, for example 30 s at most, and let the model call it again. `session/request_permission` also works, but it splits the decision between two paths, so I recommend the endpoint. Test a wait of hours before step 4.
5. **Change the journal.** Key on session, operation ID and argument hash. Refuse an ID with other arguments. On a replay of a patch, check the recorded result hash against the file. Persist process records on the worker. Keep the model-picked ID as the retry key, because all 4 lost replies with it had an exact replay, and use `claudecode/toolUseId` for the worker link. Mark each replay in the result.
6. **Correct "The real environment".** The CLI always tells the model the hub's folder, platform and shell, and this cannot be turned off. So `workspace.inspect` data must say that it overrides that message, on each turn. The worker must keep refusing absolute paths, with an error that tells the model what to do. Use a session folder name that does not look like a project. Test this first with a Windows worker.
7. **Change the Docker line.** The session host image needs a weaker container for bubblewrap. Build a narrow seccomp profile, or prefer the systemd unit. If bubblewrap cannot start, the session host must refuse to run, not run with no boundary. Landlock is a second layer only.
8. **Add supervision.** Start a watchdog with a time limit for the first `initialize` and `session/new`. Kill by process group on macOS, where `--die-with-parent` does not exist. A hung CLI does not exit by itself.
9. **Change the Mac warning.** The visible warning for the Mac sandbox must say that keychain items are not separated per profile. Do not use the Mac setup with two people's accounts.
10. **Record what the user loses.** With `tools: []` there are no subagents, no `Skill` tool and no web tools, also in Full access. The `plugins` route for skills in `claude_meta_for` stops. Skills need `skill.read` from the start, and web access needs a Consort tool if it is wanted.
11. **Keep the MCP token per session.** It is on the command line. On Linux, run the session host as its own user.

## Not tested

- A Windows worker. `t10-misc` only simulated path rules with `path.win32`. A lexical check passes `C:foo.txt`, `a.txt::$DATA`, `NUL`, `con.txt` and `a.txt.`, so the Windows worker needs its own rules. No real `CreateFile` behavior, junction or shell ran.
- A real model or a real sign-in on Linux. A new profile reports `"loggedIn": false`. I did not copy credentials, and the owner would need to sign in. A real model on Linux is therefore unproven, although the proxy path is proven.
- CLI 2.1.314 on Linux. Linux used the bundled 2.1.312.
- bubblewrap under systemd, and any narrower seccomp profile.
- A token refresh in the boundary.
- A second keychain item from inside the boundary.
- The worker restarting, with processes running.
- A real hub restart. The stand-in only answered approvals.
- A script skill, and the tools `fs.list`, `fs.search`, `process.write_stdin` and `process.signal`.
- Models other than Haiku. Tool quality on a large real task. Other harnesses.
- Managed settings under `/Library/Application Support/ClaudeCode`. The macOS file rules deny that path, but I did not plant a file there.
- A wait of more than 700 s at the endpoint.

## Other notes

- Some process on this Mac sent empty requests to the prototype's open worker ports. I did not identify it. The real worker link has credentials, so this does not affect the design.
- Side effects outside the worktree and `/tmp`: real runs wrote transcripts to `$CLAUDE_CONFIG_DIR/projects/-private-tmp-hub-exp-sessions-real{1,2,3,4}`. I removed those folders afterwards. The boundary denied all other writes to the profile. The Docker image `consort-hub-exp:bwrap` was removed.
