# Run a hub on a server

This page shows how to run a Consort hub on a Linux server: the program, the session host, the services, the address (Tailscale or HTTPS), backups and upgrades. The hub is one program and one data folder. It listens on a local address. Tailscale or a reverse proxy in front of it gives it an address with HTTPS.

There are two ways. The first is one script, `scripts/install/hub.sh`. The second is the manual steps, which the script follows.

The files for this page are in `deploy/hub/`, in `deploy/session-host/` and in the `Dockerfile` at the root of the repo.

## The quick way: hub.sh

The script installs the hub and the session host as two systemd services, gives the hub an address, pairs the session host, and prints the address and a setup code. You type both in the app (Connect to a hub).

> **Status.** `hub.sh` is new. It has a dry run, a shellcheck test and a test with a fake release, and these run in CI. A run on a real server has not been done in the build that wrote it. Try it on a spare server first, and read `scripts/install/hub.sh`. It is the same steps as the manual way below.

What you need:

- A Linux server (x64 or arm64) with systemd, and root access (or `sudo`).
- One of two addresses:
  - **Tailscale (recommended).** Tailscale runs on the server and is signed in (`tailscale up`). HTTPS certificates are on for your tailnet: an admin turns them on one time in the Tailscale admin console, on the DNS page. You need no domain name, no Caddy and no open port. A device must be on your tailnet to reach the hub.
  - **A domain name.** An A or AAAA record of the name points at the server, and ports 80 and 443 are open to the internet. Port 7600 stays closed. The script installs Caddy, which gets the certificate.

Run it with Tailscale:

```
curl -fsSL https://raw.githubusercontent.com/watzon/consort/main/scripts/install/hub.sh | sudo bash
```

Run it with a domain name:

```
curl -fsSL https://raw.githubusercontent.com/watzon/consort/main/scripts/install/hub.sh | sudo bash -s -- --domain hub.example.com
```

With neither Tailscale nor `--domain`, the script stops before it changes anything, and explains the two choices. Other options: `--version v0.2.0` for one release, `--dry-run` to print the steps and change nothing (it needs no root), and `--help`.

The script:

1. Checks that the system is Linux on x86_64 or arm64, that it runs as root, and that systemd runs. A script from a pipe cannot start `sudo` by itself, so pipe it into `sudo bash`.
2. Downloads the release and checks its SHA-256 against `SHA256SUMS`. A wrong checksum stops the script before it changes anything.
3. Installs `bubblewrap` and `git` with `apt` or `dnf` when they are missing. With neither, it stops and names them. It also installs the libraries and fonts of the preview browser (see "The preview browser"). It picks the right name on Ubuntu 24.04 and Debian 13. A library that does not install is a warning, and the script goes on.
4. Creates the users `consort` and `consort-session`, puts `consort` in `/usr/local/bin`, and installs the units `consort-hub` and `consort-session-host` (the same files as in `deploy/`).
5. Writes the public URL of the hub in `/etc/consort/hub.env`, and starts the hub.
6. Gives the hub its address. With Tailscale, it runs `consort hub reach tailscale`. With `--domain`, it installs Caddy, adds the site in `/etc/caddy/consort.caddy` and one `import` line in `/etc/caddy/Caddyfile`, and waits for the certificate. It never turns on Tailscale Funnel and never runs `tailscale serve reset`.
7. Checks that bubblewrap can make a user namespace for the session host. On Ubuntu 24.04 and later, AppArmor can stop it. The script then adds a profile for `bwrap` with the `userns` rule. If that fails, it says what to do and goes on. When AppArmor limits user namespaces, the script also adds a profile for the preview browser. The browser is not on the server yet on a first run, so the profile names its path with a `*` for the version. When the browser is there (on a second run), the script runs it as the user of the session host and draws a test page.
8. Pairs the session host, starts it, and prints the hub address and a new setup code.

**Run it again.** A second run is safe. It keeps the data, replaces the program and the units when they differ, and restarts the services only when it changed something. It refuses to install a version that is older than the one on the server. It keeps the address that the first run chose. If the first run stopped on a problem, such as HTTPS that is off for the tailnet or a domain that has no record yet, fix the problem and run the script again. It continues.

**The public URL.** With a domain, `CONSORT_PUBLIC_URL` is `https://hub.example.com`. With Tailscale, it is `http://127.0.0.1:7600`: the hub stays on loopback, and the address on the tailnet is a reach address that the hub accepts with no restart. The session host runs on the same machine and signs the public URL, so it pairs with it. A connection logo that the hub serves has the public URL in its address, so on a Tailscale hub another device shows a monogram in place of the logo. To change this, set `CONSORT_PUBLIC_URL` to the Tailscale address in `/etc/consort/hub.env`, and pair the session host again with that address.

## The manual way

Install one of these ways: a systemd service with Caddy or Tailscale (below), or Docker with Compose (see "Docker"). Do these steps as root, or with `sudo`.

### What you need

- A Linux server (x64 or arm64) with a public IP address, if you use a domain name.
- A DNS name for the hub, for example `hub.example.com`, with an A or AAAA record that points at the server. You can use Tailscale in place of a name (see "Reach the hub over Tailscale").
- Ports 80 and 443 open to the internet, with a domain name. Port 7600 stays closed: only the proxy on the same machine uses it.
- `git` on the server. The hub uses it to read skill sources. For a private skill repo, the user that runs the hub needs a way to sign in to it: a git credential helper for an `https` repo, or an ssh key in an ssh agent (`SSH_AUTH_SOCK`) and the host in `~/.ssh/known_hosts` for an `ssh` repo. Check with `git ls-remote <url>` as that user. The hub stores no password or key. The `consort` systemd unit and the Docker image do not have an ssh agent by default, so a private ssh repo needs the agent socket in the unit (`Environment=SSH_AUTH_SOCK=...`) or a read-only deploy key in the user's `~/.ssh`.

## Install the program

1. Open the latest release of Consort on GitHub. The repository name is in `RELEASES_REPO` in `crates/consort-proto/src/release.rs`.
2. Download `consort-linux-x86_64.tar.gz` (or `consort-linux-arm64.tar.gz`) and `SHA256SUMS`.
3. Check the file: `sha256sum --check --ignore-missing SHA256SUMS`.
4. Unpack it and put the program in place:

```
tar -xzf consort-linux-x86_64.tar.gz
sudo install -m 0755 consort /usr/local/bin/consort
```

## Run it as a service

The unit `deploy/hub/consort-hub.service` runs the hub as the user `consort`. It is hardened: no new privileges, a read-only system, no home folders, no devices, a short list of system calls, and the network families that the hub needs. The hub keeps its data in `/var/lib/consort`, which systemd makes for it.

```
sudo useradd --system --home-dir /var/lib/consort --shell /usr/sbin/nologin consort
sudo install -m 0644 deploy/hub/consort-hub.service /etc/systemd/system/
sudo mkdir -p /etc/consort
```

Set the public URL of your hub in `/etc/consort/hub.env`. The unit reads this file after its own `Environment` lines, so it replaces the placeholder in the unit:

```
CONSORT_PUBLIC_URL=https://hub.example.com
```

With Tailscale in place of a domain name, use `CONSORT_PUBLIC_URL=http://127.0.0.1:7600`. The address on the tailnet is a reach address, and the hub accepts it with no restart (see "Reach the hub over Tailscale").

Then start it:

```
sudo systemctl daemon-reload
sudo systemctl enable --now consort-hub
sudo systemctl status consort-hub
curl http://127.0.0.1:7600/healthz
```

`/healthz` answers `{"ok":true,...}` when the hub runs.

The unit sets `CONSORT_HUB_DATA`, `CONSORT_HUB_LISTEN` and `CONSORT_PUBLIC_URL`. The program reads the same names as the options `--data`, `--listen` and `--public-url`. Put other settings, such as `RUST_LOG=debug`, in `/etc/consort/hub.env` too.

The hub logs go to the journal: `journalctl -u consort-hub -f`.

## Run the session host

The session host holds the accounts and runs the agent CLIs. It is a separate service with its own user, because the hardened hub unit does not let a CLI or bubblewrap run. It cannot read `hub.db`, and the hub cannot read the profiles of the accounts. A session does not start when bubblewrap cannot run.

```
sudo apt install bubblewrap git
# The libraries of the preview browser: see "The preview browser" below.
sudo useradd --system --home-dir /var/lib/consort-session-host --shell /usr/sbin/nologin consort-session
sudo install -m 0644 deploy/session-host/consort-session-host.service /etc/systemd/system/
sudo -u consort consort hub session-host-code --data /var/lib/consort
sudo -u consort-session consort session-host pair <public URL> --code <code> \
  --config-dir /var/lib/consort-session-host/config
sudo systemctl daemon-reload
sudo systemctl enable --now consort-session-host
```

Pair with the public URL of the hub, exactly as you set it in `CONSORT_PUBLIC_URL` (for example `https://hub.example.com`, or `http://127.0.0.1:7600` with Tailscale). The session host signs that address, and the hub accepts only its public URL and the addresses of its reach. A session host that pairs with `http://127.0.0.1:7600` while the public URL is `https://hub.example.com` fails with `unauthorized`.

On Ubuntu 24.04 and later, AppArmor can stop bubblewrap from making a user namespace. Test it: `sudo -u consort-session bwrap --unshare-user --ro-bind / / true` must print nothing. If it fails with "No permissions to create new namespace", add a profile for `/usr/bin/bwrap` with the `userns` rule, in `/etc/apparmor.d/consort-bwrap`:

```
abi <abi/4.0>,
include <tunables/global>

profile consort-bwrap /usr/bin/bwrap flags=(unconfined) {
  userns,

  include if exists <local/consort-bwrap>
}
```

Then load it with `sudo apparmor_parser -r /etc/apparmor.d/consort-bwrap`. `hub.sh` does this. The other way is `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`, which turns the restriction off for every program on the server. The preview browser needs a profile of its own (see "The sandbox of the preview browser").

With Docker, use `deploy/session-host/docker-compose.yml`. It uses `deploy/session-host/seccomp.json`, which allows only the calls that bubblewrap needs beyond the default list. The preview browser needs the same calls for its sandbox. The image installs the libraries and fonts of the browser, so it needs no other step. Both images need a Sayso checkout beside this repo, because the workspace has a path dependency on it.

Settings > Hub shows the state of the boundary. Sign in to accounts in Settings > Providers.

### The preview browser

The session host draws a visual as an image with a headless browser, the preview browser. It is `chrome-headless-shell` from Chrome for Testing, in one version that Consort pins. The session host downloads it the first time that it draws a visual. You install no package for the browser itself.

- **Download.** It is about 125 MB, from `storage.googleapis.com` over HTTPS. The session host checks its SHA-256. The server needs outbound access to that host. The browser takes about 290 MB of disk. The first visual starts the download in the background. That `preview_visual` call fails at once, with a message that the browser installs, and the assistant tries again later.
- **Place.** `<consort home>/tools/chrome-headless-shell/<version>/`. With the service, that is `/var/lib/consort-session-host/home/tools/chrome-headless-shell/<version>/`. With Docker, it is `/data/home/tools/chrome-headless-shell/<version>/`, in the volume.
- **Platforms.** It runs on Linux with glibc (x64 or arm64). Alpine and other musl systems have no build of it. On such a system, install Chromium with the package manager, and set `CONSORT_VISUAL_CHROME` to its path (see below).

#### Libraries and fonts

The browser needs shared libraries and a font that a minimal server does not have. Without a library, the browser does not start. Without `fontconfig`, sans-serif text shows in a monospace font. Without `fonts-liberation`, a page has no glyphs.

`hub.sh` and the Docker image install these. By hand, on Debian 12 or Ubuntu 22.04:

```
sudo apt install libasound2 libatk-bridge2.0-0 libatk1.0-0 libatspi2.0-0 libdbus-1-3 \
  libexpat1 libgbm1 libglib2.0-0 libnspr4 libnss3 libx11-6 libxcb1 libxcomposite1 \
  libxdamage1 libxext6 libxfixes3 libxkbcommon0 libxrandr2 fontconfig fonts-liberation
```

On Debian 13 and Ubuntu 24.04, five names end with `t64`: `libasound2t64`, `libatk-bridge2.0-0t64`, `libatk1.0-0t64`, `libatspi2.0-0t64` and `libglib2.0-0t64`. On Ubuntu 24.04, `apt install libasound2` fails with "has no installation candidate", so use the `t64` name. On Fedora, the packages are `alsa-lib atk at-spi2-atk at-spi2-core dbus-libs expat mesa-libgbm glib2 nspr nss libX11 libxcb libXcomposite libXdamage libXext libXfixes libxkbcommon libXrandr fontconfig liberation-fonts`.

To check the libraries, run this after the browser downloaded. It prints nothing when every library is there:

```
sudo -u consort-session sh -c 'ldd /var/lib/consort-session-host/home/tools/chrome-headless-shell/*/chrome-headless-shell | grep "not found"'
```

#### The sandbox of the preview browser

The page of a visual is HTML that the assistant wrote, and it can use the network. The browser keeps it in its own sandbox, which uses user namespaces. The session host never turns the sandbox off by itself. The sandbox starts for a normal user, such as `consort-session` or the user `consort` of the Docker image. These cases need a step:

- **Ubuntu 24.04 and later.** AppArmor can stop an unknown program from making a user namespace. The browser then stops with "No usable sandbox!". Add a profile with the `userns` rule for the path of the browser, in `/etc/apparmor.d/consort-chrome-headless-shell`:

  ```
  abi <abi/4.0>,
  include <tunables/global>

  profile consort-chrome-headless-shell /var/lib/consort-session-host/home/tools/chrome-headless-shell/*/chrome-headless-shell flags=(unconfined) {
    userns,

    include if exists <local/consort-chrome-headless-shell>
  }
  ```

  Load it with `sudo apparmor_parser -r /etc/apparmor.d/consort-chrome-headless-shell`. `hub.sh` does this. The `*` is the version, so one profile fits each update. You can load the profile before the browser is on the server. The other way is the `sysctl` in the section above, which also lets the browser start.
- **Docker.** The default seccomp profile of Docker stops the browser with "No usable sandbox!". The profile `deploy/session-host/seccomp.json` allows the calls that the sandbox needs, and it is the same profile that bubblewrap needs. With it, the browser starts in the image on x64 and on arm64, with its sandbox on. If the browser or bubblewrap still stops on a host where AppArmor limits user namespaces, run the `sysctl` of the section above on the host.
- **Root.** The browser does not run its sandbox as root. The units and the image run as a normal user, so this does not happen there. If you run the session host as root, the browser does not start, and the error names `CONSORT_VISUAL_NO_SANDBOX=1`. That setting turns the sandbox off: put it in `/etc/consort/session-host.env` for the service, or use `-e CONSORT_VISUAL_NO_SANDBOX=1` for Docker. We do not recommend it. The page is untrusted HTML with network access. With no sandbox, a flaw in the browser lets a page act as the user of the session host, and that user holds the accounts of your agents. Run the session host as a normal user instead.

To test the sandbox, run this after the browser downloaded. It uses the sandbox, because it has no `--no-sandbox` flag:

```
sudo -u consort-session sh -c '/var/lib/consort-session-host/home/tools/chrome-headless-shell/*/chrome-headless-shell \
  --headless --disable-gpu --disable-dev-shm-usage --user-data-dir=/tmp/consort-browser-test \
  --screenshot=/tmp/consort-browser-test.png "data:text/html,test"; \
  rm -rf /tmp/consort-browser-test /tmp/consort-browser-test.png'
```

It prints "bytes written to file". With "No usable sandbox!", use the steps above.

#### Another browser

`CONSORT_VISUAL_CHROME` is the path of a Chrome or Chromium program, for example `/usr/bin/chromium`. When it is set, the session host uses that program in place of the managed browser, and it downloads nothing. Use it on a platform that has no Chrome for Testing build, such as musl Linux. Put it in `/etc/consort/session-host.env` for the service, or in the `environment` of the Compose file. The libraries and the sandbox of that program are your job: the same rules apply.

#### When the browser cannot run

The hub and the session host keep working. A visual stays live in the desktop app where the app can show a live page. It has no image, so the app has no picture to show in its place, for example under a menu. Where the app has no live page (Windows and Linux), a visual shows "The visual did not load" with Try again. The assistant calls `preview_visual` to see a page before it shows it. That call returns the error, for example that a library is missing or that the sandbox cannot start, so the assistant can tell you. The session host writes the reason in its log: `journalctl -u consort-session-host`, or `docker compose logs session-host`. Fix the cause with the steps above, then ask for the visual again.

## HTTPS with Caddy

The hub does not make certificates itself. A reverse proxy does it, and this is the supported way.

Why not a built-in certificate client (ACME)? It would add a second job to the hub: renewals, rate limits, port 80 for the challenge, and a place to keep certificate keys. Caddy already does this well, and one Caddyfile of four lines is all that it needs. The hub also stays a plain HTTP server that is easy to test. If you want a built-in client later, it can be added without a change for people who use a proxy.

1. Install Caddy: see [caddyserver.com/docs/install](https://caddyserver.com/docs/install).
2. Put `deploy/hub/Caddyfile` in `/etc/caddy/Caddyfile`, with `hub.example.com` changed to your name. If the file has other sites, save the hub site as `/etc/caddy/consort.caddy` and add the line `import /etc/caddy/consort.caddy` to `/etc/caddy/Caddyfile`. `hub.sh` does it this way.
3. Run `sudo systemctl reload caddy`.

Caddy gets a certificate for the name, renews it, and sends requests to `127.0.0.1:7600`. WebSocket needs no extra setting in Caddy: it passes the upgrade by itself. The two settings in the file keep long uploads and the open links of apps and workers from timing out.

If you use nginx or another proxy, it must pass the WebSocket upgrade for `/v1/client` and `/v1/worker`, must not buffer, and must not close an idle connection early. Use a read timeout of at least one hour. Set `proxy_set_header Upgrade $http_upgrade` and `Connection "upgrade"` in nginx.

One limit to know: the hub allows at most 16 connections from one address that are still in their first handshake. Behind a proxy every connection has the address of the proxy, so this limit counts all clients and workers together. An app and a worker open one connection each and finish the handshake in milliseconds, so this is rarely a problem.

## Docker

The `Dockerfile` builds the hub into a small image that runs as a user without rights. `deploy/hub/docker-compose.yml` runs the hub and Caddy together, with automatic HTTPS.

```
cp deploy/hub/.env.example deploy/hub/.env      # set HUB_DOMAIN
docker compose -f deploy/hub/docker-compose.yml up -d
docker compose -f deploy/hub/docker-compose.yml logs hub
```

The data is in the volume `hub-data`. Caddy keeps its certificates in `caddy-data`. Do not delete this volume: Caddy would ask for new certificates and can reach a rate limit.

To run the hub alone, without Compose:

```
docker build --build-context sayso=../sayso -t consort-hub .
docker run -d --name consort-hub --restart unless-stopped \
  -p 127.0.0.1:7600:7600 -v consort-hub-data:/data \
  -e CONSORT_PUBLIC_URL=https://hub.example.com consort-hub
```

The image has a health check on `/healthz`.

## Pair the first app

On the first start, the hub makes a setup code. It shows in the log. To make a new one at any time:

```
sudo -u consort consort hub setup-code --data /var/lib/consort
```

With Docker: `docker exec <container> consort hub setup-code`.

Open the Consort app, choose On a server, and enter the hub URL (`https://hub.example.com`, or the address of Tailscale) and the setup code. A code works once and expires after 10 minutes.

To add a worker, open Settings > Workers > Add worker in the app. The limits of a worker change in the same page, and no file on the worker needs an edit. The dialog gives one command for the system of the worker. See [install.md](install.md).

A hub and a worker can run on the same server. Install the worker as a user service (`consort worker install`, see install.md) for the user that should own its files, and pair it with `https://hub.example.com`. Run the worker as a normal user, not as the `consort` user of the hub.

## Reach the hub over Tailscale

An app cannot change Tailscale on a hub that runs as a service or in Docker. It only shows the state. Run these on the server, as root or as the owner of the data folder. Pass `--data`: without it the command opens the default folder of the user that runs it, which is not the data folder of the service.

```
sudo consort hub reach status --data /var/lib/consort
sudo consort hub reach tailscale --data /var/lib/consort
sudo consort hub reach off --data /var/lib/consort
```

`consort hub reach off --only tailscale` and `--only this-network` turn off one way. Consort changes only its own Serve entry, on the first free port of 443 and 8443, and never turns on Funnel. The hub must have started once. The running hub reads the change with no restart. If you ran a command as root, make the data folder belong to the `consort` user again: `sudo chown -R consort:consort /var/lib/consort`.

Tailscale needs HTTPS certificates on for the tailnet. If they are off, the command prints a link for an admin. Then the hub address is `https://<machine>.<tailnet>.ts.net`, with `:8443` when another service holds port 443. This is the address that the app and `consort worker install` use.

## Back up

Back up the whole data folder (`/var/lib/consort`, or the Docker volume). It holds:

| File | What it is |
|---|---|
| `hub.db` with `hub.db-wal` and `hub.db-shm` | Every conversation, task, memory note and setting. SQLite, in WAL mode |
| `blobs/` | Attachments, channel files, artifacts and skill files |
| `hub-secret.key` and `composio.key` | The key that protects the Composio key, and the Composio key itself |

Do not copy `hub.db` while the hub runs. Use the online backup of SQLite, which is safe at any time:

```
sudo -u consort sqlite3 /var/lib/consort/hub.db ".backup '/var/backups/consort/hub.db'"
sudo rsync -a /var/lib/consort/blobs/ /var/backups/consort/blobs/
sudo cp -p /var/lib/consort/hub-secret.key /var/lib/consort/composio.key /var/backups/consort/
```

Blobs never change after the hub writes them, so a copy of `blobs/` that is a little older than the database is fine. Keep a copy off the server.

The details of each file are in [data-model.md](data-model.md). The backup and restore commands of the hub itself are planned in phase 3 of `FINISH-PLAN.md` (Settings > Hub). When your version has them, they replace the steps above.

To restore, stop the hub, put the files back, make the files belong to the `consort` user, and start the hub.

## Upgrade

1. Back up (see above).
2. Run `sudo consort hub update`. To install one version, run `sudo consort hub update --version 0.8.0`. A server that `hub.sh` installed can also run `hub.sh` again.

The command downloads the release from the fixed repository, checks its SHA-256 and replaces the program. As root with systemd, it also restarts `consort-hub` and `consort-session-host` when they run. Otherwise it says what to restart. It refuses an older version. Settings > Hub shows this command when a newer version exists.

A hub from before this command has no `consort hub update`. Upgrade it by hand one time:

1. Download the new release and check it, as in "Install the program".
2. `sudo install -m 0755 consort /usr/local/bin/consort`
3. `sudo systemctl restart consort-hub`

The unit file sets `CONSORT_INSTALL_KIND=service`, and the Dockerfile sets `docker`. The hub reports this in `hub.status`, and with no variable it reports `manual`. If you installed the unit before this variable, add the line to it, so that the app shows the right command.

With Docker, `consort hub update` refuses to run. A new image replaces the program: `git pull`, then `docker compose -f deploy/hub/docker-compose.yml up -d --build`.

The hub changes its database by itself when it starts. A change only goes forward, so keep the backup until you know that the new version works. An old program cannot read a database that a newer one changed.

`GET /healthz` shows the version of the hub. Update the hub. The hub sets the version for the rest:

- **Workers follow the hub.** When a worker connects to a newer hub, the hub names its version. The worker downloads that release from the fixed repository, checks it, and restarts when its work ends. It never moves to an older version. Settings > Workers shows the state.
- **The app waits for its hub.** An app update that cannot talk to the connected hub stays ready but waits, and the sidebar says so. When the hub is new enough, Restart works. An app that is too old for the hub stays open with its saved data and says so.
- **The session host** uses the same program as the hub on the same machine. `consort hub update` and `hub.sh` restart both services.

A software version never blocks a connection. Only the protocol range does: each release speaks the current protocol and the one before it. So the order of updates matters little for a release that keeps the protocol.

## Check the setup

- `curl https://hub.example.com/healthz` answers from any computer.
- `ss -ltn` on the server shows port 7600 on `127.0.0.1` only.
- The app connects and shows no "Cannot reach the hub" line.
