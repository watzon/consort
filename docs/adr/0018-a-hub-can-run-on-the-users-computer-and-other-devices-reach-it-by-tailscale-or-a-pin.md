# A hub can run on the user's computer, and other devices reach it by Tailscale or a pin

A hub can run on the same Mac or Linux computer as the app. First run offers this and a hub on a server as two equal choices. Other devices reach a hub through Tailscale first, or through a pinned certificate on This network. Plain `http` stays for development. This replaces two rules of ADR 0015 for a local hub: "a Mac hub is for development only" and the session host as its own user.

## Context

- ADR 0015 puts the accounts and the agents on the hub machine, and says that a Mac hub is for development only. On Linux the session host runs as its own user, which needs admin rights.
- A user with one Mac should not need a server to try Consort. The user decided on 2026-10-31 that a hub on the same computer as the app is valid, and that the app offers it at the start.
- A hub on a server has to be reachable from a phone. ADR 0002 keeps every peer dial-out to the hub, so reach is about how the hub is addressed. It does not change who dials.
- A test on a real tailnet on 2026-10-31 (Tailscale 1.104.4) showed that `tailscale serve` gives a loopback hub a real certificate, passes a WebSocket and passes a 256 MiB upload. See [ONBOARDING-PLAN.md](../ONBOARDING-PLAN.md).

## Decision

**First run.**

- The first screen of a new app has two choices: On this computer and On a server. They are equal. Neither is a hidden default. The user can change the choice later in Settings > Hub.
- The setup gate is a function of the hub state. It saves no step. A user who quits in the middle comes back to the same screen.

**A hub on this computer.**

- `consort local` runs the hub, the session host as its child, and a worker in one per-user service, on a free loopback port. It needs no admin rights. It updates with the app.
- The session host and the worker run as the same user as the hub. So the hub process can read the account profiles. A session still runs in the boundary. This replaces the one-user rule of ADR 0015 for a local hub only. A hub on a Linux server keeps the session host as its own user.
- A Mac hub is a valid hub for its owner. It is no longer for development only. The Mac boundary (`sandbox-exec`) stays what it is.
- Windows has no local path. `boundary::detect` returns `UnsupportedSystem` there, so no session can start. A user can install a hub in WSL by hand. The app can offer this later.

**Reach.**

- Tailscale is the first choice. The hub stays on loopback. `tailscale serve` gives it an address on the tailnet, on the first free port of 443 and 8443. The hub never turns on Funnel, never changes an entry that is not its own, and never runs `tailscale serve reset`.
- This network is the second choice. The hub makes a self-signed certificate and listens with TLS on a second port. A pairing code or link carries the SHA-256 of the certificate (the pin), and a device accepts no other certificate.
- Plain `http` stays for development only. The official paths never use it.
- The hub accepts the signatures of its public URL and of each reach that is on. The wire protocol does not change.
- A hub on a server cannot change Tailscale itself. The install script, or `consort hub reach` as root, does it.
- Tailnet membership is not sign-in. A new device pairs with a code.

**A hub on a server.** `scripts/install/hub.sh` installs the hub and the session host on Linux as two systemd services, and uses Tailscale or a domain with Caddy. The manual steps stay in [deploy.md](../deploy.md).

**Versions.** A software version never blocks anything. Only the protocol range does. Each release speaks the current protocol and the one before it. The hub names the version of its workers, and an app does not update past its hub.

## Known limit: the Mac keychain

On a Mac, `mac_profile` gives a session with a Claude plan login read and write access to all of `~/Library/Keychains`. On a local hub this is the real login keychain of the user. `sandbox-exec` does not separate keychain items per profile.

The user knows this and deferred it on 2026-10-31. It is not solved here. It must have an answer before a public release of the Mac install. Until then Settings > Hub on a Mac keeps its warning. The text of that warning (`boundary_warning` in `consort-client`) still says "development sandbox" and "Use a Linux server for real work". It changes when the user decides.

## Consequences

- A user can start with no server and no admin rights.
- A local hub and a server hub differ in who owns the profiles. The docs and Settings > Hub must say which kind a hub is. `hub.status` reports it as the install kind.
- A local hub on a Mac shows the user's login keychain to a session. Do not call the Mac install ready for the public until the user decides.
- A user on Windows must use a hub on a server, or WSL by hand.
- Reach adds a TLS listener, a pin verifier in `consort-tls`, and a field in the pairing invite.
- The pairing limit counts all devices on a tailnet as one address, because Serve makes traffic look like it comes from loopback. The limit only stops guessing, so this is acceptable.

## Considered options

- Keep the Mac hub for development only. Rejected by the user on 2026-10-31.
- Make the app run its own hub with no service. Rejected: the hub must stay up when the app quits, and a service already exists for the worker.
- Funnel for public reach. Rejected: it puts the hub on the public internet.
- A built-in ACME client. Rejected before. Caddy already does this, and Tailscale needs none.
- Trust a certificate authority on This network. Rejected: a pin needs no authority and no install on the device.
- Make Windows work with a weaker boundary. Rejected: with no boundary, no session starts.
