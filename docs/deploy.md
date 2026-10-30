# Run a hub on a server

This page shows how to run a Consort hub on a VPS: the program, the service, HTTPS, backups and upgrades. The hub is one program and one data folder. It listens on a local address. A reverse proxy in front of it serves HTTPS.

The files for this page are in `deploy/hub/` and in the `Dockerfile` at the root of the repo.

## What you need

- A Linux server (x64 or arm64) with a public IP address.
- A DNS name for the hub, for example `hub.example.com`, with an A or AAAA record that points at the server.
- Ports 80 and 443 open to the internet. Port 7600 stays closed: only the proxy on the same machine uses it.
- `git` on the server. The hub uses it to read skill sources.

Install one of these ways: a systemd service with Caddy (below), or Docker with Compose (see "Docker").

## Install the program

1. Open the latest release of Consort on GitHub. The repository name is in `RELEASES_REPO` in `crates/consort-client/src/update/mod.rs`.
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
sudo systemctl edit consort-hub
```

In the editor that opens, set the public URL of your hub:

```
[Service]
Environment=CONSORT_PUBLIC_URL=https://hub.example.com
```

Then start it:

```
sudo systemctl daemon-reload
sudo systemctl enable --now consort-hub
sudo systemctl status consort-hub
curl http://127.0.0.1:7600/healthz
```

`/healthz` answers `{"ok":true,...}` when the hub runs.

The unit sets `CONSORT_HUB_DATA`, `CONSORT_HUB_LISTEN` and `CONSORT_PUBLIC_URL`. The program reads the same names as the options `--data`, `--listen` and `--public-url`. Put other settings, such as `RUST_LOG=debug`, in `/etc/consort/hub.env`. The file is optional.

The hub logs go to the journal: `journalctl -u consort-hub -f`.

## HTTPS with Caddy

The hub does not make certificates itself. A reverse proxy does it, and this is the supported way.

Why not a built-in certificate client (ACME)? It would add a second job to the hub: renewals, rate limits, port 80 for the challenge, and a place to keep certificate keys. Caddy already does this well, and one Caddyfile of four lines is all that it needs. The hub also stays a plain HTTP server that is easy to test. If you want a built-in client later, it can be added without a change for people who use a proxy.

1. Install Caddy: see [caddyserver.com/docs/install](https://caddyserver.com/docs/install).
2. Put `deploy/hub/Caddyfile` in `/etc/caddy/Caddyfile`. Change `hub.example.com` to your name.
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
docker build -t consort-hub .
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

Open the Consort app, enter the hub URL (`https://hub.example.com`) and the setup code. A code works once and expires after 10 minutes.

To add a worker, open Settings > Workers > Add worker in the app. The dialog gives one command for the system of the worker. See [install.md](install.md).

A hub and a worker can run on the same server. Install the worker as a user service (`consort worker install`, see install.md) for the user that should own its files, and pair it with `https://hub.example.com`. Run the worker as a normal user, not as the `consort` user of the hub.

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
2. Download the new release and check it, as in "Install the program".
3. `sudo install -m 0755 consort /usr/local/bin/consort`
4. `sudo systemctl restart consort-hub`

With Docker: `git pull`, then `docker compose -f deploy/hub/docker-compose.yml up -d --build`.

The hub changes its database by itself when it starts. A change only goes forward, so keep the backup until you know that the new version works. An old program cannot read a database that a newer one changed.

`GET /healthz` shows the version of the hub. Update the hub first, then the workers, then the apps.

## Check the setup

- `curl https://hub.example.com/healthz` answers from any computer.
- `ss -ltn` on the server shows port 7600 on `127.0.0.1` only.
- The app connects and shows no "Cannot reach the hub" line.
