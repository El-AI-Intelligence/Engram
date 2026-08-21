# Changelog

All notable changes to Engram by El AI Intelligence are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the project
versions with [SemVer](https://semver.org/).

## [Unreleased]

## [0.1.1] — 2026-08-21

Third-party QC security review remediation (2026-08-20) and a product
rename: the product is now presented as **Engram by El AI Intelligence**
across the CLI, vault UI, landing page, installers, MCP tool descriptions,
and documentation (binary and package names are unchanged). Full scope:
daemon trust model, UI XSS, relay tenant isolation, KDF hardening, and
misc hardening — all findings reproduced against HEAD before fixing.

### Security

- **Key-handoff mint gating** — `POST /sync/key-handoff/start` now requires
  the daemon's admin credential (`ENGRAMD_API_KEY`, or a daemon-generated
  token persisted 0600 to `{vault}/.handoff-token` when the env var is
  unset). Browser redeem stays token-only. (`090e997`)
- **WS/CORS origin pinning** — `/ws/events` upgrades are origin-gated, and
  the CORS layer no longer echoes arbitrary `localhost:*` ports; exact
  origins only, `ENGRAM_CORS_ORIGINS` for development. (`b6e9e81`)
- **SPA XSS** — every template interpolation in the vault UI is escaped
  (incl. `'`), hrefs are `encodeURIComponent`-encoded, and a CSP
  (`csp_headers` middleware + meta tag) plus `X-Content-Type-Options` and
  `X-Frame-Options: DENY` are set on daemon responses. (`17943b9`)
- **Import hardening** — imported memories are validated (plain ids,
  bounded content/project lengths) and rejected items are reported in the
  response. (`17943b9`)
- **Relay tenant isolation** — account API keys are scoped per vault;
  unscoped ("account-wide") keys minted before this release are
  policy-denied with a 403 that asks the device to re-link. Pair/link
  mints carry a client-derived `vault_id`; key mints are refused for
  vaults the account doesn't own. (`e88a630`)
- **Passkey rate limits + login caps** — register start/finish and login
  start are rate-limited; login-passkey lookups are filtered and capped
  per account. (`e88a630`)
- **SMTP STARTTLS-mandatory** — the relay's reset-mail transport uses
  `Tls::Wrapper` (opportunistic downgrade removed). (`e88a630`)
- **Vault-wrap fresh-password gate** — storing a key wrap now re-verifies
  the account password; the SPA prompts when needed. (`e88a630`)
- **KDF v2 vault ids** — the passphrase→vault-id derivation uses a new
  domain salt and higher Argon2id cost (96 MiB); unpinned devices probe
  the relay and converge on whichever derivation (v2 or legacy v1) already
  EXISTS, creating fresh vaults under v2. Pinned ids are untouched; a
  rejected api_key aborts sync (daemon stays up). (`247ca39`)
- **Pair onto manually-named vaults** — `engram pair --vault-id <id>`
  overrides the passphrase-derived vault id and pins it into
  `config.json`, so devices can join teams whose vault id was named by
  hand (`engram join`) — passphrase convergence can never reach those. (`9b70f31`)
- **MCP capture honesty** — `engram_capture` checks the daemon's HTTP
  status and reports rejection bodies (401/403) instead of a false
  "captured successfully". (`8cdb5d1`)
- **Purge date validation** — `before_date` must be RFC3339; a malformed
  string (which previously matched the whole vault via lexical SQL
  comparison) is rejected with 400 before anything is deleted. (`8cdb5d1`)

### Fixed

- Password reset responses are uniform (`sent: true` whenever SMTP is
  configured) — no account enumeration via reset. (`e88a630`)
- Privacy audit dashboard returns real `sync_enabled`/`local_only` values
  instead of placeholders. (`8cdb5d1`)

### Docs

- `SYNC.md` — per-vault key scoping, legacy-key 403 re-link guidance, KDF
  v2 convergence behavior, `ENGRAM_CORS_ORIGINS` usage. (this release)

## [0.1.0] — 2026-08-19

First public release. Passphrase-encrypted local vault, end-to-end
encrypted multi-device sync (dumb relay), passkey accounts + one-click
device linking/pairing, weekly digest, MCP server, browser vault UI,
Windows/macOS daemons, Docker image, Homebrew formula, apt/deb packaging.
