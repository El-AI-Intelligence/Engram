# Deploying Engram by El AI Intelligence to engram.ellmstack.dev

Production runbook for hosting the Engram by El AI Intelligence (landing page, vault UI,
and `engramd` API) behind Caddy on a single Linux server.

## Architecture

```
browser
   │  HTTPS :443 (automatic TLS via Let's Encrypt)
   ▼
┌──────────────────────────────────────────────────────┐
│ Caddy 2                                              │
│  /            → static files  /srv/engram/landing    │  (public)
│  /app*        → static files  /srv/engram/vault      │  (public shell — branded login screen)
│  /health, /memories*, /context*, /consolidate*,      │
│  /analytics*, /config, /export, /import, /ws*,       │
│  /annotations*, /searches*, /privacy*, /sync*        │
│             → reverse_proxy 127.0.0.1:8787 ──────────┼──► engramd daemon --vault /var/lib/engram/vault
│               (+ injected Authorization header)      │     (systemd; API only, no static files)
└──────────────────────────────────────────────────────┘
```

`engramd` binds to loopback only (`127.0.0.1:8787`) and exposes only the JSON
REST API — it does not serve static files. Caddy serves the vault SPA
statically at `/app` as a **public shell** (the bundle holds no data — a
branded login screen gates it), enforces HTTP basic auth on the API paths,
and injects the bearer token the API requires, so the SPA's same-origin
`fetch()` calls carry no API key.

## Server topology (which box is which)

Engram by El AI Intelligence runs on **two separate Hetzner VPSes** — kept split on purpose:

| Box | Hetzner | Public IP | Serves | Runs |
|---|---|---|---|---|
| **Site box** | `ubuntu-4gb-hel1-1` | 204.168.163.161 | `engram.ellmstack.dev` (landing, vault UI at `/app`, installer binaries) | Caddy + `engramd` (vault daemon on 127.0.0.1:8787) |
| **Sync relay** | server 125572177 (CX23) | 138.199.144.93 | `sync.ellmstack.dev` (E2E sync relay) | Caddy + `engramd-sync` (127.0.0.1:8788) |

Both are 2 vCPU / 4 GB / 40 GB. The site box also hosts **guardrail**, so it
cannot be retired — that kills the "merge everything onto the relay" idea
(no cost savings, and the relay keeps its minimal attack surface). The relay
runbook is `deploy/sync-relay.md`; this file covers only the site box.

The site box has **no git checkout** — `/root/engram/` is a plain copy:
rsync `ui/`, `scripts/`, `deploy/`, and `target/release/` binaries from a dev
machine, then run `./deploy/engram/deploy.sh -y`. Note the systemd unit runs
`/root/engram/engramd` while deploy.sh installs to `/usr/local/bin/` — copy
the unit's binary by hand first, so the restart boots the new build.

The exact procedure (from a dev machine with the repo at HEAD, then on the box):

```bash
# dev machine — NO trailing slashes: a trailing slash flattens the CONTENTS
# into /root/engram/ and silently leaves /root/engram/ui/ (what deploy.sh
# reads) stale. This is the #1 way the site ends up serving yesterday's UI.
rsync -a --chown=root:root -e ssh ui deploy scripts root@204.168.163.161:/root/engram/
scp target/release/engramd target/release/engram target/release/engramd-mcp \
    root@204.168.163.161:/root/engram/target/release/

# box — backup the unit's binary (house convention), swap, then deploy
ssh root@204.168.163.161
cp -p /root/engram/engramd /root/engram/engramd.bak-$(date +%Y%m%d)
install -m 0755 /root/engram/target/release/engramd /root/engram/engramd
cd /root/engram && ./deploy/engram/deploy.sh -y
```

`install` swaps the binary while the daemon runs (rename, no "Text file
busy"); the deploy.sh restart then boots the new build. The real vault on
this box lives at `/root/engram/vault` (the unit is host-specific — the
generic `/var/lib/engram` paths below are for a fresh host).

**2026-08-18 incident:** the deploy was run on the wrong machine (a home
workstation, not this box) and the site kept serving the old `main.js` —
symptom: served file hash ≠ deployed file hash while the site still
answers. Check `/srv/engram/vault/js/main.js` ON THE SITE BOX first; if the
box file is new but the served one is old, someone deployed elsewhere.

## Cloudflare

`engram.ellmstack.dev` is **grey-cloud (DNS only)** — flipped 2026-08-16
after repeated stale-UI incidents: while orange-cloud proxied, the edge
caches `main.js` and keeps serving it after deploys, and Cloudflare
terminates TLS (seeing vault traffic, undermining the end-to-end story).
The A record points straight at the site box and Caddy serves its own
Let's Encrypt cert; `dig +short engram.ellmstack.dev` → 204.168.163.161.
The vault SPA also serves `Cache-Control: no-cache`, so browsers revalidate
on every load.

If the record is ever re-proxied (orange): expect stale-UI reports again —
purge the zone cache after each deploy, or flip back to grey. The flip was
done via the Cloudflare API (zone `ellmstack.dev`
`010caaa5c0050af0606fa00f1395fc11`, A record
`3a6786e6ced92607be9d00c2b272f049`, `proxied: false`); Claude Code has the
`cloudflare@cloudflare` plugin installed (OAuth'd MCP), so DNS/cache changes
can be made with the API instead of the dashboard.

## Prerequisites

- **DNS:** an A record for `engram.ellmstack.dev` pointing at the server,
  with ports 80 and 443 reachable (required for Let's Encrypt issuance).
  Prefer DNS-only (grey cloud) — see "Cloudflare" above.
- **Software:** Caddy 2, systemd, rsync.
- **User and directories:**
  ```bash
  useradd --system --home /var/lib/engram --shell /usr/sbin/nologin engram
  install -d -m 0755 /srv/engram/landing /srv/engram/vault
  install -d -m 0750 -o engram -g engram /var/lib/engram/vault
  install -d -m 0750 -o engram -g engram /etc/engram
  ```
- **Binary:** `engramd` installed at `/usr/local/bin/engramd`
  (e.g. `cargo build --release -p engramd && install target/release/engramd /usr/local/bin/`).
  The daemon serves only the API; the vault SPA is static files under
  `/srv/engram/vault`, served by Caddy at `/app`.

## Secrets

### `/etc/engram/engramd.env` (mode 0600, owned by `engram`)

```bash
ENGRAMD_API_KEY=<long-random-token>        # required — bearer token for the API
ENGRAM_PASSPHRASE=<vault passphrase>       # optional — vault encryption / sync
ENGRAM_CORS_ORIGINS=                       # optional — usually empty behind Caddy
```

### Caddy environment

Caddy substitutes env vars when it loads the config. Provide them via a
systemd override (`systemctl edit caddy`):

```ini
[Service]
Environment=ENGRAMD_API_KEY=<same token as engramd.env>
Environment=ENGRAM_UI_USER=<username>
Environment=ENGRAM_UI_PASS_HASH=<bcrypt hash>
```

Generate the basic-auth password hash with:

```bash
caddy hash-password
```

## How the two auth layers interact

1. The landing page at `/` and the vault SPA shell at `/app` are **public**.
   The SPA renders a branded login screen (route `#/login`) instead of
   Caddy's native basic-auth popup.
2. All API paths (`/health`, `/memories*`, …) stay behind Caddy's HTTP
   **basic auth** (`ENGRAM_UI_USER` / bcrypt hash). The login form sends an
   explicit `Authorization: Basic base64(user:pass)` header — browsers never
   show the native popup when a request already carries credentials, and
   Caddy strips the `WWW-Authenticate` challenge from API 401s, so a
   rejected login shows an inline error on the login screen instead. The
   strip lives in a `handle_errors 401` route (see the Caddyfile comment):
   basic_auth's 401 travels Caddy's error path, which bypasses the `header`
   directive's write-time wrapper — verified on Caddy 2.11, a site-level
   `header @api -WWW-Authenticate` does NOT strip basic_auth's own 401s.
   `/auth/ui-logout`'s 401 is a `respond`, not an error, so it keeps its
   deliberate challenge.
3. For proxied API requests, Caddy **replaces** the `Authorization` header
   with `Bearer {$ENGRAMD_API_KEY}` before forwarding to `engramd`
   (`header_up` in `deploy/caddy/engram.Caddyfile`).
4. `engramd`'s middleware (`crates/engramd/src/auth.rs`) requires
   `Authorization: Bearer <key>` on every non-`/health` route when
   `ENGRAMD_API_KEY` is set, with constant-time comparison and a 100 req/s
   rate limit.

So the vault UI's same-origin fetches (`fetch('/health')` etc. — `API = ''`
in `ui/engram-vault/js/main.js`) work without embedding any API key in the
SPA. The API key lives only in server-side config.

The SPA keeps the vault credentials only in `sessionStorage`
(`engram-vault-creds`) — per-tab, gone when the tab closes, deliberately no
"keep me signed in". If a previous basic-auth session left credentials in
the browser's auth cache, the login screen probes `/health` headerless once
and auto-enters; that self-eliminates after the first sign-out.

**Sign-out:** basic auth has no server-side session — the browser caches
the credentials. The SPA's "Sign out" calls `/auth/ui-logout`, which Caddy
answers with `401` + `WWW-Authenticate: Basic realm="restricted"`; the
matching challenge makes Chrome/Firefox discard the cached credentials
(Safari may keep the cache — closing the tab always works). The SPA then
clears `sessionStorage` and returns to the login screen without a reload.

**Known consequence:** `WebSocket()` cannot set headers, so `/ws/events`
always 401s behind the gate; the UI's existing 5s polling + 8s reconnect
fallback takes over permanently (the stream badge shows "polling" instead
of "live").

## First-run checklist

1. DNS A record live and ports 80/443 open.
2. Prereqs above done (user, dirs, binary, `engramd.env`, Caddy env override).
3. Main `/etc/caddy/Caddyfile` imports the site config:
   ```caddy
   import /etc/caddy/engram.Caddyfile
   ```
4. Run the deploy script from the repo:
   ```bash
   sudo deploy/engram/deploy.sh          # or -y to skip the prompt
   ```
   It rsyncs `ui/landing/` → `/srv/engram/landing/` and
   `ui/engram-vault/` → `/srv/engram/vault/` (with `--delete`), installs the
   Caddyfile and systemd unit, runs `systemctl daemon-reload`,
   `systemctl enable --now engramd`, validates the Caddy config, and reloads
   Caddy.
5. Verify:
   ```bash
   journalctl -u engramd -n 50                 # daemon started, vault opened
   curl -s http://127.0.0.1:8787/health        # direct API health check
   curl -sI https://engram.ellmstack.dev/       # 200, valid TLS cert
   curl -sI https://engram.ellmstack.dev/app/   # 200 — public SPA shell (login screen)
   curl -sI https://engram.ellmstack.dev/health # 401, and NO www-authenticate header
   curl -sI -u user:pass https://engram.ellmstack.dev/health  # 200 — the SPA's login path
   ```

TLS certificates are issued and renewed automatically by Caddy via
Let's Encrypt — no certbot or cron needed.

## Operations

- **Logs:** `journalctl -u engramd -f` (daemon) and `journalctl -u caddy -f`
  (access/TLS).
- **Backup:** the vault is SQLite under `/var/lib/engram/vault/`. Stop the
  service (or use SQLite's online backup) and copy the directory:
  ```bash
  systemctl stop engramd
  tar -C /var/lib/engram -czf engram-vault-$(date +%F).tar.gz vault
  systemctl start engramd
  ```
- **Upgrade:** install the new `engramd` binary, then re-run
  `sudo deploy/engram/deploy.sh -y` (re-syncs static assets, reinstalls the
  unit, restarts the service, reloads Caddy).
- **Streaming endpoints:** `/context/stream` is SSE — Caddy is configured
  with `flush_interval -1` so events are not buffered. `/ws/events` is a
  WebSocket, which Caddy upgrades transparently.

## Local development

`engramd` serves only the API, so run the SPA from any static file server and
let it talk to the daemon on `127.0.0.1:8787` (CORS allows localhost origins;
`API = ''` in `ui/engram-vault/js/main.js` means the SPA expects the API on
the same origin, so use a tiny proxy — see below):

```bash
cargo run -p engramd -- daemon --vault ./engram-data          # API on :8787
python3 ui/engram-vault/server.py                             # static + mock API on :8787 (offline demo)
```

The mock server (and a local `engramd` without the Caddy gate) applies no
basic auth, so the login screen's headerless `/health` probe always
succeeds and the SPA auto-enters — the login flow itself is only
exercisable behind the real Caddy gate.

For live-API development, serve `ui/engram-vault/` statically and proxy
non-file requests to `127.0.0.1:8787` (any dev proxy works; Caddy with the
same matchers as `deploy/caddy/engram.Caddyfile` minus auth also works).

## Related files

- `deploy/caddy/engram.Caddyfile` — Caddy site config
- `deploy/systemd/engramd.service` — systemd unit
- `deploy/engram/deploy.sh` — idempotent deploy script
- [INSTALL.md](INSTALL.md) — end-user installation
- [SYNC.md](SYNC.md) — multi-device sync and its own proxy example
