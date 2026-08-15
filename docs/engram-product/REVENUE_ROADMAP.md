# Engram Revenue Roadmap

**Date:** 2026-08-12
**License:** MIT (core stays open — the paid layers are services, and the open core is what earns the trust that sells them)

## Principles

1. **Local-first + E2E stays.** Zero-knowledge encryption is the moat. Every paid layer is optional — a self-hoster gets the full product forever.
2. **Flat pricing, no seats, no metering.** Customers pay one price per plan. No per-seat or per-request economics, for us or for them.
3. **Flat infra costs.** No per-request third-party APIs in any hot path. Embeddings stay local (ONNX/candle). Any cloud LLM use is BYO-key or local-model.
4. **The core never closes.** Only the control plane (accounts, billing, team admin) is proprietary — and it's thin enough that it lives outside this repo anyway.

## What exists today (verified 2026-08-12)

- `engramd` — local daemon with ONNX embeddings, FTS5 + vector + QEM holographic memory, REST + WebSocket API, MCP server (`engramd-mcp`), privacy controls (audit/purge/retention)
- E2E sync **client** in `engramd` (`/sync/status`, `sync_server_url` config) — edit propagation via `modified_at` push cursor (2026-08-14)
- `engramd-sync` — sync **relay server**: stateless "dumb pipe" for encrypted blobs, HMAC verification, token-bucket rate limiting, tombstones with 30-day retention, API-key auth. *Built; running locally, public deploy pending.*
- **Shared-vault v0 (teams) shipped 2026-08-14:** `vault_id` + passphrase multi-device sync, device roster endpoint, `/teams/status`, Settings "Sync & Team" panel. Zero-knowledge core untouched — the server still only sees device IDs and blob counts.
- Live site: landing + vault UI + API behind Caddy basic auth
- Guardrail platform experience (WebAuthn accounts, admin dashboard, roles) to reuse for the control plane

---

## Layer 1 — Engram Cloud Sync

The Obsidian play: sell the cloud layer of a local-first product. The relay already exists; this layer is deployment + accounts + billing.

**Offering:** free = 1 device + 1 GB; Solo = 5 devices + 10 GB + 30-day restore points.

| Milestone | Work | Est. |
|---|---|---|
| 1.1 Deploy relay | **Shipped 2026-08-15:** `engramd-sync` live at `sync.ellmstack.dev` (Hetzner, Caddy + LE TLS, systemd + sandbox, nightly snapshots). Verified two-device bidirectional E2E round trip through the public URL; relay DB audit shows only IDs/vector clocks/ciphertext/HMAC — zero plaintext. Runbook: `deploy/sync-relay.md`. | done |
| 1.2 Accounts + devices | WebAuthn accounts (reuse guardrail's auth), per-account API keys, device registry in the vault UI, quota flags (devices, bytes) | 2–3 wks |
| 1.3 Billing | Stripe flat plan **Solo $4/mo**; webhook → quota flags (relay was designed for this: "billing is handled by a separate service") | 1 wk |
| 1.4 Restore points | Server-side snapshot markers; "restore from N days ago" in the vault UI | 1 wk |

**Cost to run:** 1 VPS + block storage — flat, roughly $15–25/mo regardless of user count.

**Why first:** builds the accounts + billing + quota foundation that Layers 2 and 3 reuse.

---

## Layer 2 — Team Memory

The revenue core. "The AI that remembers your organization."

**Offering:** shared org vault — decisions, context, tribal knowledge. Admin console with roles, retention, compliance. Flat pricing: **Team $29/mo** (≤ 10 members), **Org $99/mo** (unlimited members, SSO). No per-seat math, ever.

**v0 substrate shipped (2026-08-14):** the MIT core already does shared
vaults — passphrase-scoped teams with a device roster, `/teams/status`, and a
Settings "Sync & Team" panel. What's missing is exactly the private control
plane: accounts, roles, revocation, audit (2.1/2.2). The zero-knowledge moat
is validated: the paid layer adds administration without touching the
encrypted core.

| Milestone | Work | Est. |
|---|---|---|
| 2.1 Org vaults | Collections + ACLs in `engramd` config, invite/join flow, shared workspace in the vault UI | 3 wks |
| 2.2 Admin console | Roles, retention policies, full audit/export, GDPR deletion (reuse the existing privacy routes), login via the Layer 1 account system | 3 wks |
| 2.3 Enterprise readiness | SAML/OIDC, data residency options, on-prem offer (control plane runs on their infra) | later |

**Cost to run:** Postgres + control plane on the same VPS class — flat, ~$25–50/mo.

---

## Layer 3 — One Memory Everywhere

The consumer endgame: your memory follows you across Claude, Cursor, ChatGPT, Copilot.

**Offering:** **Personal $5/mo** flat — includes Solo sync. Hook: the weekly digest ("what your AI learned about you this week").

| Milestone | Work | Est. |
|---|---|---|
| 3.1 MCP everywhere | **Shipped 2026-08-15:** `engram mcp install` writes/merges configs for Claude Desktop / Cursor / Windsurf (creates where the app is installed, prints snippets otherwise), `engram mcp status` checks binary + daemon + editors, `claude mcp add` for Claude Code, repo `.mcp.json`, capture tools report skipped duplicates honestly, search uses the daemon's hybrid default — docs in MCP.md. Web-vault read-only polish deferred | done |
| 3.2 Weekly digest | **Shipped 2026-08-15:** `GET /digest/weekly` + Digest tab in the vault UI + `engram digest` CLI. Deterministic local core (new/reinforced/fading stats, themes clustered from the vault's own embeddings, quarantine counts) — zero per-request cost; AI-written prose opt-in per request via the user's own OpenAI-compatible endpoint (BYO-key, never automatic; local Ollama qualifies). Email/push delivery remains future | done |
| 3.3 Consumer onboarding | **Shipped 2026-08-15:** `engram onboarding` (5-minute vault → first memory → running daemon; passphrase via env, never argv) + consumer landing reframe + real one-command installer (`curl install.sh \| bash` — SHA-256-verified release binaries for all three bins served from the live site, staged by deploy.sh; verified end-to-end: install → onboarding → daemon → retrievable memory). Browser extension remains (future) | done — extension deferred |

**Cost to run:** digest runs locally or BYO-key — no metered costs; extension is dev time only.

---

## Sequencing

```
Layer 1 (sync + accounts + billing)   →   Layer 2 (team memory, the revenue core)   →   Layer 3 (consumer)
```

Layer 1 pays for the infra and de-risks accounts/billing. Layer 2's control plane powers Layer 3's consumer accounts. The MIT core never changes.

## What stays MIT vs. what stays private

- **MIT (this repo):** `axiom-engram`, `engramd`, `engramd-sync`, `engramd-mcp`, both UIs, deploy configs. Self-hosters get everything — that's the trust engine.
- **Private services (never in this repo):** the control plane (accounts, billing, team admin), Stripe integration, digest delivery. Thin layers by design.

## Open questions

- **Brand/domain for the cloud:** keep `engram.ellmstack.dev` or move to its own domain before charging money?
- **Capacity tiers:** are 1 GB / 10 GB the right flat buckets? (Capacity is the only meter we keep — flat tiers, not per-GB billing.)
- **Digest model:** answered — deterministic local core by default, BYO-key prose opt-in per request (2026-08-15).
- **Org size cap:** "≤ 10 members" needs a soft limit in the app — hard limits feel un-flat; how to enforce gracefully?
