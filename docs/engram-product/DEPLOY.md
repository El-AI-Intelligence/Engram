# Deploying Engram to engram.ellmstack.dev

Production runbook for hosting the Engram Memory Vault (landing page, vault UI,
and `engramd` API) behind Caddy on a single Linux server.

## Architecture

```
browser
   │  HTTPS :443 (automatic TLS via Let's Encrypt)
   ▼
┌──────────────────────────────────────────────────────┐
│ Caddy 2                                              │
│  /            → static files  /srv/engram/landing    │  (public)
│  /app*        → static files  /srv/engram/vault      │  (basic_auth)
│  /health, /memories*, /context*, /consolidate*,      │
│  /analytics*, /config, /export, /import, /ws*,       │
│  /annotations*, /searches*, /privacy*, /sync*        │
│             → reverse_proxy 127.0.0.1:8787 ──────────┼──► engramd daemon --vault /var/lib/engram/vault
│               (+ injected Authorization header)      │     (systemd; API only, no static files)
└──────────────────────────────────────────────────────┘
```

`engramd` binds to loopback only (`127.0.0.1:8787`) and exposes only the JSON
REST API — it does not serve static files. Caddy serves the vault SPA
statically at `/app`, enforces HTTP basic auth, and injects the bearer token
the API requires, so the SPA's same-origin `fetch()` calls carry no API key.

## Server topology (which box is which)

Engram runs on **two separate Hetzner VPSes** — kept split on purpose:

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
the unit's binary by hand (stop the service first, or you get "Text file
busy").

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

1. The browser hits any `/app` or API path → Caddy challenges with HTTP
   **basic auth** (`ENGRAM_UI_USER` / bcrypt hash). The landing page at `/`
   is public.
2. For proxied API requests, Caddy **replaces** the `Authorization` header
   with `Bearer {$ENGRAMD_API_KEY}` before forwarding to `engramd`
   (`header_up` in `deploy/caddy/engram.Caddyfile`).
3. `engramd`'s middleware (`crates/engramd/src/auth.rs`) requires
   `Authorization: Bearer <key>` on every non-`/health` route when
   `ENGRAMD_API_KEY` is set, with constant-time comparison and a 100 req/s
   rate limit.

So the vault UI's same-origin fetches (`fetch('/health')` etc. — `API = ''`
in `ui/engram-vault/js/main.js`) work without embedding any API key in the
SPA. The API key lives only in server-side config.

**Sign-out:** basic auth has no server-side session — the browser caches
the credentials. The SPA's "Sign out" calls `/auth/ui-logout`, which Caddy
answers with `401` + `WWW-Authenticate: Basic realm="restricted"`; the
matching challenge makes Chrome/Firefox discard the cached credentials, so
the next `/app` load re-prompts (Safari may keep the cache — closing the
tab always works).

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
   curl -sI https://engram.ellmstack.dev/      # 200, valid TLS cert
   curl -sI https://engram.ellmstack.dev/app/  # 401 without basic auth
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

For live-API development, serve `ui/engram-vault/` statically and proxy
non-file requests to `127.0.0.1:8787` (any dev proxy works; Caddy with the
same matchers as `deploy/caddy/engram.Caddyfile` minus auth also works).

## Related files

- `deploy/caddy/engram.Caddyfile` — Caddy site config
- `deploy/systemd/engramd.service` — systemd unit
- `deploy/engram/deploy.sh` — idempotent deploy script
- [INSTALL.md](INSTALL.md) — end-user installation
- [SYNC.md](SYNC.md) — multi-device sync and its own proxy example
