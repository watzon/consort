# Design notes: onboarding

These are the Paper designs for the first run of Consort: setup in the app, and the first conversation in Tutti. The Paper file is `01M6342D512B1CKH0D29HMDN8V`, page Onboarding. All artboards are dark and 1440 px wide.

**Status (2026-11-01).** All six phases of [../ONBOARDING-PLAN.md](../ONBOARDING-PLAN.md) are built and not committed. The artboards and the built app agree, with the differences in decisions 829 to 930 of [../DECISIONS.md](../DECISIONS.md) (for example 844, 855, 887, 903, 904 and 915). Paper S2 and the built app agree: S2 shows one install command and the setup code, the app shows `curl -fsSL .../scripts/install/hub.sh | bash`, and `hub.sh` exists, prints the code and uses the same address as the app. The text below is the record of the design. Where it says that something does not exist yet, a phase of the plan built it.

The example data is the same in all artboards. The user is Chris, the account is Claude, the worker is MacBook Pro, and the channels are sayso and homelab.

## The main decision

The app wizard holds only what the assistant cannot do for itself: get a hub, and sign in to one account. The assistant needs an account before it can speak, so the Tutti tools `add_account` and `start_sign_in` cannot do the first sign-in.

Everything else is a conversation in Tutti, in the same style as the guided start of a new channel (`F6N-0`, `FDX-0`, `FJC-0`, and M14 in [PROACTIVITY-PLAN.md](../PROACTIVITY-PLAN.md)).

The Three voices mark is the progress indicator. There is no stepper. The blue dab is the hub, the green dab is the account, and the gold dab is the assistant. A dab lights when its stage starts. All three are dark on W1, and the gold dab lights when Tutti opens.

## Artboards

| Artboard | ID | What it shows |
|---|---|---|
| O1 Who sees what | `T78-0` | The three stages with their dabs, and the table "What opens": which hub state opens which screen. |
| W1 Where should Consort run | `TE9-0` | The first screen on a new app: On this computer (Install here) or On a server (Connect to a hub). |
| W2 Installing on this computer | `TEA-0` | Four steps with no input, what stays running, and the folder. |
| W3 Install states | `TEB-0` | Windows has no local option, Linux needs bubblewrap, the hub does not start, a hub is already here, the hub on this computer has stopped, and what follows the install. |
| W4 Settings, Hub, on this computer | `TEC-0` | Start when I sign in, Reach from other devices, Move to a server, Connect to another hub, Remove from this computer. |
| V1 The hub is too old for this app | `TB3-0` | The app stays open with its saved channels, and a card says what to do. |
| V2 Version states | `TED-0` | An update that waits for the hub, a newer hub version, a hub too old at pairing, a restart with running tasks, workers that follow the hub, and an app too old for the hub. |
| R1 Reach from other devices | `TKW-0` | Tailscale and This network. Tailscale is the primary choice, and the card says what goes in public certificate logs. |
| R2 Tailscale on, add a device | `TKX-0` | The address, Turn off, and Add a device with a QR code and a setup code. The QR pattern is a placeholder. |
| R3 Reach states | `TKY-0` | Tailscale not installed, not connected, HTTPS off, port 443 in use, Linux permission, a server hub, a device off the tailnet, an address that someone removed, and This network on. |
| S1 Connect to your hub | `S65-0` | Hub URL and setup code. Opens from W1. The build also needs a Back action to W1, which is not drawn. |
| S2 Start a hub on your server | `S73-0` | One install command and the setup code. The command is `scripts/install/hub.sh`, which phase 6 built. |
| S3 Choose a provider | `S8J-0` | One provider for the first account, and Do this later. |
| S4 Sign in | `S7T-0` | The sign-in of the CLI on the hub machine: open the page, and paste the code. Tabs for Claude plan, Console and API key. |
| S5 Setup when something goes wrong | `T75-0` | Cannot reach the hub, setup code refused, connecting, no session host, installing the tool, install failed, code refused or expired, the first answer waits, the first answer fails. |
| T1 Tutti, first greeting | `SC7-0` | The first turn of the assistant after the first sign-in. |
| T2 Question 1, who you are | `SJ4-0` | The answer goes to global memory. |
| T3 Question 2, services and connect card | `SLT-0` | The connect card, with a field for the Composio key the first time. |
| T4 Question 3, worker card | `SOI-0` | The new worker card. Tutti skips this question if a worker is online. |
| T5 Question 4, channels appear | `ST7-0` | The answer creates channels, and they appear in the sidebar. |
| T6 The close | `SVW-0` | What was done, what the assistant does by itself, Be quieter, and first tasks as chips. |
| T7 Not now | `SYL-0` | The user skips the guide. |
| T8 No account yet | `T1A-0` | Tutti when no account is signed in. Do this later opens it. It also covers all accounts signed out. |

## Decisions

- The hub state picks the screen, not a saved step. If the hub has no signed-in account, the app opens the provider step. So a user who quits in the middle comes back to the same step, and a second app on a hub with an account goes to Tutti with its history and no greeting.
- The setup has no name step. The default account name is the short name of the provider ("Claude"). The user can rename it later. Tutti gets a default model automatically, and the composer chip shows it.
- The greeting is a live model turn, not a canned message. So it also proves that the account and the session host work. If it fails, Tutti shows the in-chat error (E1) with Try again.
- The greeting comes one time for each hub. A hub with history never gets it.
- Tutti asks four questions, one at a time: who you are, your services, where work runs, and what you work on. T5 asks no fifth question. The close follows in the same turn.
- The first message that the user types takes priority over the guide.
- "Not now" means one more offer tomorrow, through a follow-up. "No, I look around myself" means no more offers.
- The close says openly that the assistant looks in by itself and sends a morning brief, and offers "Be quieter". Tutti is at level Normal, and the assistant can only set a quieter level.
- The app asks for OS notification permission at the close, because this permission is for each client.
- The chips of the guide follow the limits of `post_message`: 4 suggestions at most, and 40 characters for each label.

## What the designs need from the backend

These do not exist yet:

1. A wake event for Tutti, like `channel_created`. It fires one time when the first account signs in and Tutti has no messages, with a persisted flag. Today `channel_created` returns false for Tutti, and `bootstrap` seeds Tutti with no wake.
2. A client setup gate, derived from hub state. Today `onboarding.rs` goes to `Page::Main` right after pairing.
3. A worker card and a Tutti tool, proposed name `add_worker`. Today `worker.pairing_code` is client API only. A deployed hub has no worker by default, and no task can run without one.
4. A Composio key field in the connect card. The assistant never sees the key.
5. The name of the user, stored on the hub. Today each client makes it from `$USER`, in `consort-client/src/hub/mod.rs`, and that value can be wrong. Tutti asks for the name in T2.
6. An empty state for Tutti with no account (T8). Today `render_empty_timeline` says only that the channel has no messages.
7. A local install command, a service for all three parts, more than one origin, and the Tailscale and network reach below.
8. `scripts/install/hub.sh` and `consort hub update`.
9. The version rules below: a second protocol version kept in support, a mismatch error with versions, a new client connection state, a release file with the protocol range, an update hold, and a worker update command.

## Where the hub runs

The user decided on 2026-10-31 that a hub on the same computer as the app is valid, and that the app offers this choice at the start.

- W1 has two equal choices and no primary button. Neither path is a hidden default, and the user can change the choice later in W4.
- The app runs the install with no input: it starts the hub, the session host and a worker for this computer, and pairs itself and them. So this path has no setup code, and Tutti skips its question about where work runs.
- The hub listens only on loopback at first. It uses a free port, because `scripts/dev.sh` already uses 7600.
- One per-user service runs all three parts, as `consort dev` does today. `consort-cli/src/service.rs` already writes a launchd agent, a systemd user unit and a Task Scheduler task for the worker. It needs no admin rights.
- The app carries the `consort` program in its bundle, and copies it to the Consort folder for the service. This replaces a download at install time. The 0.1.0 release has a `consort` file of 17 MB and a macOS app file of 18 MB, so the app grows to about 35 MB. A download would add a failure step and a way to get a program that differs from the app. The macOS bundle script also signs and notarizes helpers, and the release workflow does not sign the separate `consort` file. If this choice changes, the app must download the release with its own version, not the latest release.
- `scripts/install/hub.sh` does the same job on a server, so the server path also has one command.

### Limits of the local path

The user reviewed these on 2026-10-31.

1. **The Mac boundary.** ADR 0015 says a Mac hub is for development only, so a new ADR must replace this line. On a Mac, `mac_profile` gives a session with a Claude plan login read and write access to all of `~/Library/Keychains`. On a local hub this is the real login keychain of the user. The user does not treat this as a blocker now. It is deferred.
2. **Windows.** `boundary::detect` returns `UnsupportedSystem` on Windows, so no session can start. W1 shows that the local path is not available on Windows. A user can install the hub inside WSL by hand. The app can offer this later.
3. **One user.** ADR 0015 runs the session host as its own user on Linux, which needs admin rights. A local hub runs all parts as the user. So the hub process can read the account profiles. A session still runs in the boundary. The new ADR must accept this for a local hub.

## Reach from other devices

The user asked to settle this next, with Tailscale first. The hub stays dial-in for every peer, so ADR 0002 does not change. Tailscale stays optional.

### Tailscale

- **The hub stays on loopback.** `tailscale serve --bg --https=<port> http://127.0.0.1:<hub port>` gives the hub a `https://<machine>.<tailnet>.ts.net` address with a real certificate. The current client accepts this address with no change, and the phone needs no special network rule. The hub never turns on Funnel.
- **Server hubs get the same path.** With Tailscale, a server needs no DNS name, no Caddy and no open ports. `hub.sh` uses it when it finds Tailscale. This is the shortest path to the setup that the user prefers.
- **The hub never touches other Serve entries.** This Mac already has a Serve entry on port 443. So the hub reads `tailscale serve status --json` first. If 443 is in use, it uses 8443. It turns off only its own entry, and it never runs `tailscale serve reset`.
- **The hub finds the state by itself.** It shells out to the `tailscale` program, and looks in the service `PATH`, `/usr/local/bin`, `/opt/homebrew/bin` and `/Applications/Tailscale.app/Contents/MacOS/Tailscale`. `tailscale status --json` gives the state, the name and the HTTPS capability. No Rust binding to Tailscale is used, because `unsafe_code` is forbidden and the program already does this job.
- **A hub on a server cannot change Tailscale itself.** The hardened service has no rights for it. The install script does it as root, and the app shows the state and a command (R3).
- **Tailnet membership is not sign-in.** A new device still pairs with a code. Serve also makes traffic look like it comes from `127.0.0.1`. The hub does not trust loopback for authentication, so this adds no bypass. The pairing limit then counts all tailnet devices as one address. This is acceptable, because the limit stops guessing.

### The code changes

- **More than one origin.** `Settings` has one `origin`, and `net/ws.rs` and `net/blobs.rs` check signatures against it. The hub keeps a list: the public URL and the address of each reach that is on. It checks a signature against each entry. The wire protocol does not change. An address leaves the list when its reach turns off, so a peer still cannot pass a challenge from another hub.
- **This app keeps its address.** The saved client data is tied to `hub_url` (`persist.rs`). So existing clients and the local worker stay on `127.0.0.1`, and lose nothing.
- **Pairing uses a reachable address.** `worker_install_command` and the QR code use `hub_url`. On a local hub, this is `127.0.0.1`, which is wrong for another device. The Add worker dialog and Add a device must use the address of a reach that is on. If no reach is on, they send the user to R1.
- **A clear error.** A client with a `.ts.net` address that cannot connect says to open Tailscale (R3).

### This network

The client already allows plain `http` to private addresses and `.local` names, and the worker only warns. The official path does not use plain `http`. On a shared network, anyone can read the traffic and the setup code. After the sign-in, they can also change frames, and this includes approvals.

- The hub makes a self-signed certificate and listens with TLS on a second port. It uses `<hostname>.local` as its address.
- The QR code or a pairing link carries the SHA-256 of the certificate. The client pins it. So the user must scan or paste, and cannot only type the address.
- This costs more than Tailscale: a TLS listener in the hub, a custom verifier in the client link, the blob client and `consort-link`, and a new field in `PairingInvite`. A worker install command also needs the pin. So it comes after Tailscale.
- Plain `http` stays for development, with its warning.
- A client keeps one address for now. A list of routes, as T3 Code has, can come when a user needs both this network and another route on one device.

### Not tested

- This Mac runs Tailscale 1.104.4 with MagicDNS and the HTTPS capability on. I only read its state, and changed nothing.
- Test these on a real tailnet before the build: a WebSocket and a 256 MiB upload through Serve, the output of `tailscale serve` when HTTPS is off (the design expects a link to pass on), port 8443, and the operator rule on Linux.
- The Tailscale pages did not load here. The certificate log note comes from a search summary of their docs.

## When versions differ

Findings in the code today:

- The hub closes a link with `protocol_mismatch` when no common version exists. The client treats this as a lost link and tries again without end. The user sees only Offline. The code must not use the `Refused` path, because that path deletes the saved data of the hub.
- `deploy.md` says to update the hub, then workers, then apps. The app updates itself by default, so it goes first.
- `SUPPORTED_PROTOCOL_VERSIONS` has one version, so a bump breaks every older peer at once.
- Only two enums in `consort-proto` have a fallback for a value from a newer peer. So a new enum value can fail on an older client, with no protocol bump.
- A worker updates only when someone runs its install script again. The hub has no way to ask.

Rules:

1. Software versions never block anything. Only the protocol range does. Mobile needs this, because a store update can take days.
2. Each release speaks the current protocol and the one before it. A change that adds an optional field or a new method needs no bump. A contract test checks that an older client can read new values.
3. A hub on this computer updates with the app. Restart does these steps: backup, replace the program, restart the service, restart the app. If tasks are running, the app asks first (V2). The app and this hub then cannot differ.
4. A server hub stays under the control of its owner. `hub.status` reports how it was installed (service, Docker or by hand), and the app shows the right command (V2). One-click update can come later. The hardened service cannot write to `/usr/local/bin`, so it needs a separate helper.
5. The hub sets the version for its workers and its session host. The hub names a version, and each worker downloads that release from the fixed repository, checks it, and restarts when its work ends. A worker that the hub refuses with `protocol_mismatch` reads the hub version from the error and does the same. The hub never sends a program or an address.
6. The app does not update past its hub. Each release publishes its protocol range in a small file. If the new version cannot talk to the connected hub, the update stays ready but waits (V2).
7. If a mismatch still happens, the error carries the versions of the hub. The client gets a new connection state that says who is too old. The app stays open with its saved data, keeps the outbox, and tries again slowly (V1). Saved data stays. A new pairing shows the same reason (V2).

## Open questions

- The Mac keychain limit is deferred by the user (2026-10-31). It must have an answer before a public release of the Mac install. See ADR 0018.
- The file name `about-you.md` in T2 is an example. The memory path is not decided.
- Not drawn: light mode and the compact width (600 px). The setup column is 448 px, so it fits at 600 px, but tall steps must scroll in a short window. Chips must wrap.
- `hub.sh` has never run on a real server, and nobody has tested AppArmor handling on a real Ubuntu 24.04.
- A Tailscale hub shows another device a monogram in place of a connection logo, because the logo address has the public URL (decision 920).

## Settled

These were open before the build. The decisions are in ADR 0018 and in `DECISIONS.md`.

- A hub on the same computer is valid, and W1 offers both choices as equals.
- Reach: Tailscale first, This network with a pinned certificate second, plain `http` for development only. Tailscale is optional, and the hub never turns on Funnel.
- The versions rules (the protocol range, the hub names the version of its workers, the app does not update past its hub).

## Differences between the plan and the build

- `consort local run` pairs the session host and the worker. The plan said that `consort local install` pairs them (decision 864).
- The notification permission moved from phase 4 to phase 5 (decision 894). It is a line above the composer after the guide closes (916).
- The list of accepted origins is the store table `hub_reach`, not a list in `Settings` (846).
- The setup gate shows S3 on "no account can run, no history, and not Do this later", not on "a key and no signed-in account" (879).
- `hub.sh` holds its units inside the script and sets the public URL in `/etc/consort/hub.env` (917, 918).
