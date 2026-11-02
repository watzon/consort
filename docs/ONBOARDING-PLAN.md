# Onboarding plan

This plan builds first run from end to end: where the hub runs, setup in the app, the first conversation in Tutti, reach from other devices, and version handling.

The designs and the reasons are in [notes/design-onboarding.md](notes/design-onboarding.md). The Paper file is `01M6342D512B1CKH0D29HMDN8V`, page Onboarding. Read the note before you start a phase. Where the note and this plan differ, this plan is right.

## Verified facts

A test on a real tailnet on 2026-10-31 with Tailscale 1.104.4 on macOS showed these:

- `tailscale serve --bg --yes --https=8443 http://127.0.0.1:<port>` starts at once and prints the address.
- The certificate is from Let's Encrypt, with TLS 1.3.
- A WebSocket upgrade works through Serve, and data flows both ways.
- A 256 MiB POST works through Serve.
- Serve adds `X-Forwarded-For` and `Tailscale-User-Login` headers.
- `tailscale serve --yes --https=8443 http://127.0.0.1:<port> off` removes only that entry. Other entries stay.
- `tailscale serve status --json` lists the ports under `TCP` and the addresses under `Web`.

A test of the build on the same tailnet on 2026-11-01 showed these:

- `consort hub reach tailscale` on a local hub chose 8443, because another Serve entry had 443. The `/healthz` route answered at the tailnet address.
- A second copy of the desktop app on the same computer paired with a setup code at the tailnet address. It connected through 8443 and showed S3.
- `consort hub reach off` removed only the 8443 entry. The entry on 443 stayed.

Not tested: a second device on the tailnet, the output when HTTPS is off for the tailnet, and the operator rule on Linux. Code for the HTTPS output and the operator rule must not depend on an exact text. Show the output of the program when it does not match.

## Phases

Each phase ends with `scripts/check.sh` for the crates that it changes, and a new test for each new behavior. Add a `--mock --screen <name>` screen for each new screen. Do not commit.

### Phase 1: versions

**Status: built, not committed.** Differences from the plan are in decisions 829 to 845. The worker follows the hub through `worker.update` and the mismatch error.

1. A `protocol_mismatch` error carries the hub version and its protocol versions. A hub also sends them on a failed pairing.
2. A new `Connection::Incompatible` state in the client says which side is too old. It never takes the `Refused` path, keeps the saved data and the outbox, and retries slowly.
3. The desktop app shows V1, and the pairing error in V2.
4. `SUPPORTED_PROTOCOL_VERSIONS` has a documented rule: the current version and the one before it.
5. The release workflow publishes `release.json` with the version and the protocol versions. The updater reads it, and holds an update that cannot talk to the connected hub. The sidebar shows the hold (V2).
6. `hub.status` reports how the hub was installed: `local`, `service`, `docker` or `manual`.
7. Settings > Hub shows a newer version and the command for the install kind (V2). `consort hub update` exists for a service install: it downloads the named or latest release, checks the SHA-256, and replaces the program. It tells the user to restart the service, or does so when run as root with systemd.
8. A worker follows the hub. The hub names a version in a new command, or in the mismatch error. The worker downloads that release from the fixed repository, checks it, replaces the program and restarts when its work ends. It never downgrades. Settings > Workers shows the state (V2).

### Phase 2: reach

**Status: built, not committed.** Differences: the list of origins is the store table `hub_reach`, not a list in `Settings` (846). A hub on a server or in Docker cannot turn on This network, and the command line has no way to do it (849). Decisions 846 to 859.

1. The accepted origins are the public URL and the rows of the store table `hub_reach`, so `consort hub reach` counts with no restart (decision 846). WebSocket and blob signatures pass against any entry. A test proves that an origin outside the list fails.
2. A new hub module `reach`: it finds the `tailscale` program, reads `status --json` and `serve status --json`, and turns Serve on and off on the first free port of 443 and 8443. It never touches other entries and never runs `reset`. A trait over the program makes this testable with no Tailscale.
3. Client API: `hub.reach_status`, `hub.reach_set` and the address for pairing. A hub that cannot change Tailscale (a service or Docker install) answers with its state only.
4. Settings > Hub gets the page R1, R2 and the states in R3. Add a device uses the reach address. The Add worker dialog uses it too, and sends the user to R1 when the hub has only a loopback address.
5. A client with a `.ts.net` address that cannot connect shows the error in R3.
6. This network: the hub makes and keeps a self-signed certificate, and listens with TLS on a second port. `PairingInvite` gets an optional `tls_sha256`, and the client, the blob client and `consort-link` pin it. Plain `http` stays for development.

### Phase 3: local install

**Status: built, not committed.** Differences: `consort local run` pairs the session host and the worker at its start, and `install` waits for them to connect. The plan said that `install` pairs them (864). The command set also has `check`, `start`, `stop`, `login` and `upgrade` (867). The files are in `local/` and `bin/` of the Consort folder (860). Decisions 860 to 878.

1. `consort local install`, `status`, `uninstall`, `run` and `setup-code`. `run` starts the hub, the session host and a worker in one service, as `consort dev` does. It uses a free port on loopback, and writes a state file with the address. `install` pairs the parts and prints JSON with the address and a setup code. Reuse the plans in `consort-cli/src/service.rs`.
2. A pre-check says whether this computer can run agents: `boundary::detect`. Windows gets no local path.
3. `scripts/bundle-macos.sh` and the release workflow put `consort` in the app. The install copies it to the Consort folder. An update replaces it, restarts the service after a backup, and asks first when tasks are working (V2).
4. `consort-client` gets a local hub module: it finds the bundled program, runs the install, reads the state file, starts the hub, and detects a hub that stopped. `desktop` calls it and never spawns anything itself.
5. `hub.status` reports `local`. Settings > Hub shows W4: Start when I sign in, Reach, Move to a server, Connect to another hub, Remove.
6. Move to a server uses the existing backup.

### Phase 4: setup in the app

**Status: built, not committed.** Differences: the setup gate shows S3 when no account can run, the hub has no history and the user did not choose Do this later, and not on "no account at all" (879). Page `Setup` holds W1 to S5, and S1 stays on page `Onboarding` (882). The notification permission moved to Phase 5 (894). Decisions 879 to 894.

1. A derived setup gate in `consort-client`: no key and no local hub gives W1. A key and no signed-in account gives S3. A signed-in account gives Tutti. The gate holds no saved step.
2. The screens W1, W2, W3, S1, S2, S3, S4 and S5, with the mark as the progress. Reuse the code of `add_account.rs` and `settings_providers.rs`, as a screen and not a dialog.
3. Do this later opens T8.

### Phase 5: the first conversation

**Status: built, not committed.** No real model has run the guide. The notification permission is a line above the composer after the guide closes (916). Decisions 895 to 916.

1. A hub wake event for Tutti, like `channel_created`. It fires one time when the first account signs in and Tutti has no messages, with a persisted flag. A hub with history never gets it.
2. The coordinator prompt holds the guide in T1 to T7: four questions, one at a time, with chips. It skips the worker question when a worker is online. The first message of the user takes priority.
3. The user name is on the hub. `set_user_name` or memory holds it, and the client shows it. Tutti asks for it.
4. A worker card and a tool `add_worker`. The card makes the code and shows the command.
5. A Composio key field in the first connect card. The assistant never sees the key.
6. T8: Tutti with no signed-in account. Phase 4 built the hook, and this phase writes its words.
7. The app asks for the OS notification permission at the close of the guided start.

### Phase 6: server install and docs

**Status: built, not committed.** `scripts/install/hub.sh` has never run on a real server. It has a dry run, shellcheck, and tests of its functions with a fake release, and it ran against a fake `systemctl` in an Ubuntu 24.04 container. ADR 0018 records the decisions. Decisions 917 to 930.

1. `scripts/install/hub.sh` installs the hub and the session host on Linux, finds Tailscale and uses it, and prints a setup code. Test it with `--dry-run`.
2. A new ADR replaces the Mac line and the one-user rule of ADR 0015 for a local hub, and records reach.
3. Update `CONTEXT.md`, `docs/architecture.md`, `docs/protocol.md`, `docs/data-model.md`, `docs/install.md`, `docs/deploy.md`, `docs/BUILD-PLAN.md`, `docs/DECISIONS.md` and the context menu list in `CLAUDE.md` where they changed.
