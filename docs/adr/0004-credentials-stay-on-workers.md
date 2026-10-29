# Model credentials stay on workers

Superseded in part by [ADR 0010](0010-worker-accounts-sign-in-and-tags.md). Logins still stay on workers, but the user signs in from the app, each account has its own profile folder, and an API key travels once through the hub to the worker.

Harness logins and model API keys stay in each harness's own files on each worker. The hub and the clients never receive them. Anthropic's terms forbid an app that collects Claude plan credentials or routes requests through them, but they allow a user to sign in to the unmodified Claude Code binary with their own plan. OpenAI's Sign in with ChatGPT gives each host its own ID, which also fits one login per machine.

## Consequences

- The user signs in on each worker with the harness's own command.
- A coordinator can run only on a worker where its harness is signed in. For the assistant to stay up when the home network is down, that harness must also be signed in on the hub machine.
- Before a public release, ask Anthropic to confirm that a hub that starts Claude Code sessions on the user's machines is acceptable.
