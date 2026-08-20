# Engram Account Authentication — Architecture, Best Practices, Pitfalls

Reference for the account system (milestone 1.2) and a reusable playbook
for future projects. Written after the signup flow's first real-user run,
which surfaced four failures that unit tests and wire checks never caught.

## The flow (as built)

```
Sign up  POST /auth/signup {email, password}
  → relay creates account + hashes password (Argon2id m=19MiB t=2 p=1,
    own salt — deliberately DIFFERENT params from the wrap KDF so a leaked
    login hash yields no key material)
  → mints opaque session token (sha256 at rest, 7-day TTL, server-side
    revocable row in `sessions`)
  → SPA generates account key A (32B, client-only), shows a 12-word BIP39
    recovery phrase ONCE, then PUTs two AES-GCM envelopes:
      PUT /account/wraps/recovery  A wrapped under Argon2id(phrase)
      PUT /account/wraps/password  A wrapped under Argon2id(password)
  → relay stores only ciphertexts (zero-knowledge preserved)

Sign in  POST /auth/signin → same opaque session
  → SPA derives the wrap key from the password, unwraps A in memory
  → open-by-default vaults unwrap their composite K (enc‖hmac‖vault_id)
    silently; locked vaults keep the passphrase path
  → no wraps yet (aborted signup) → re-show the phrase gate ("setup")

Passkey  optional second factor: WebAuthn assertion mints the same opaque
  session, but yields no A → open vaults prompt the account password once

Reset    request → single-use 30-min token (SMTP or operator CLI) → confirm
  → revokes ALL sessions, rehashes password. Old password wrap unreadable
  → recovery phrase unwraps A → re-wrap under the new password

Vault lock toggle = wrap row add/delete (`vault_key_wraps`). No memory
re-encryption ever. Lock policies (session/close/5|15|60 min) are
client-side per vault.
```

Key properties: server-side sessions → instant revocation ("kick all out"
= delete rows); A and every vault key live only in JS memory; the relay
can decrypt nothing; recovery phrase is the sole account-key recovery.

## Best practices (researched 2026-08) and where we sit

Sources that informed the design review: [MojoAuth — passkey user journeys
in a hybrid system](https://mojoauth.com/blog/passkey-user-journeys-hybrid-auth-system),
[Oak Security — authentication hardening
guide](https://academy.oaksecurity.io/resources/authentication-hardening-guide),
[JWT vs sessions — a complete
guide](https://dev.to/yuktisays/jwt-vs-sessions-a-complete-guide-to-modern-web-authentication-security-flow-and-best-practices-1nf2),
[Clerk — React authentication: from protected routes to
passkeys](https://clerk.com/articles/react-authentication-from-protected-routes-to-passkeys),
[Privacy Guides — email
security](https://github.com/privacyguides/privacyguides.org/blob/main/blog/posts/email-security.md?plain=1#2).

| Practice | Consensus | Engram |
|---|---|---|
| Session storage | Never in localStorage; HttpOnly SameSite cookie or in-memory | **Deliberate exception:** SPA is on `engram.ellmstack.dev`, relay on `sync.ellmstack.dev` — cookies don't cross domains, and adding a BFF was out of scope. Bearer in localStorage, mitigated by opaque server-side sessions, 7-day TTL, revocation, 401-clears-token everywhere. Revisit if the origins ever merge |
| Token type | Opaque server-side session over JWT | ✅ opaque rows in `sessions` |
| Revocation | Server-side delete = instant | ✅ all sessions deleted on reset, "sign out", admin kicks |
| Recovery | A ladder, not one rung; email is the weakest link | ✅ 12-word phrase shown once (mandatory), email only for resets; passkey optional rung. No email OTP at all |
| Passkey UX | Returning users must never see a password field first (conditional UI) | ⚠️ v1 has a passkey button on the signin form; conditional-mediation autofill is the known follow-up |
| State-changing methods | Non-GET verbs | ✅ PUT/DELETE for wraps — see pitfall #1 |
| Rate limits | On reset and signin | ✅ buckets: signup 3/hr, signin 10/5min, reset 3/hr per email |
| Session hardening | Regenerate after auth, timeouts | ✅ fresh token per signin; 7-day absolute TTL |

## Pitfalls that actually bit us (each cost real user-facing breakage)

1. **CORS `allow_methods` missing PUT.** The relay's `CorsLayer` listed
   GET/POST/DELETE; the wrap routes are PUT. Every earlier step in the flow
   used an allowed method, so tests, curl (no preflight), and the first
   user steps all passed — then the browser's preflight blocked the wrap
   PUTs with a bare `Failed to fetch`. **Rule: when adding a route, check
   the method against the CORS allow-list in the same commit.** A curl
   with an `Origin:` + `Access-Control-Request-Method:` preflight OPTIONS
   is the exact repro.

2. **Headerless auto-enter probes self-sabotage.** A legacy "are cached
   creds still good?" `/health` probe on the login screen relied on the
   browser NOT attaching cached basic-auth. Browsers silently attach
   cached credentials to same-origin requests, so the probe always got
   200 and bounced the user into the console — the account form was
   unreachable ("logs me right in on hard refresh"). **Rule: never gate
   UX on the absence of browser credential caching — it is invisible and
   uncontrollable.**

3. **Token presence ≠ valid session.** Any forward-on-token-presence
   logic will bounce-loop on a dead token (revoked server-side, e.g. by a
   kick). Every place that uses a stored token must treat a 401 as
   *clear the token and render signed-out* — including menu chips and
   mount-time checks, not just the login screen.

4. **Phantom signed-in UI from `.catch(() => showSignedIn())`.** A catch
   that renders the signed-in state on ANY error shows a signed-in menu
   for a 401. Branch on `e.status === 401` and fall to signed-out.

5. **Aborted signup leaves a recoverable state, but only via signin.**
   Signup creates the account + session before the phrase gate. If the
   gate fails or is abandoned, the account exists with no wraps — signup
   again gives 409 `email_taken`, and an existing session shows the
   account banner (no re-entry to the gate). Signin detects "no wraps" and
   re-shows the phrase gate (`setup`). Revoking the stale session is the
   clean way to steer the user back to the signin form.

6. **sqlite3 CLI vs the app's foreign keys** (from the relay deploy log):
   the CLI defaults to `PRAGMA foreign_keys=OFF`, so manual `DELETE FROM
   accounts` doesn't cascade. Always `PRAGMA foreign_keys=ON;` first.

7. **Links printed with the bare host land on the marketing page.** The
   SPA lives at `/app`; the landing page ignores `#/handoff/...`
   fragments. Every handoff link "worked" (page loads fine, no error) and
   the daemon logged zero redeems — the failure was completely silent.
   **Rule: any CLI/SMS/email link that targets a hash route must include
   the real SPA path.** The `engram handoff` `--site` default is
   `https://engram.ellmstack.dev/app` for exactly this reason.

8. **Account key A is per-tab JS memory — a link opened in a NEW tab has
   no A.** The handoff route prompted for the account password on every
   new-tab open, then a bad link failed AFTER the typing. Fixed three
   ways: (a) redeem the one-time token BEFORE any password can be asked,
   so a dead link fails with nothing typed; (b) same-origin
   `BroadcastChannel` (`engram-account-key`) shares A tab→tab — still
   memory-only, never at rest, dies when the last tab closes; (c) the
   password prompt is now the last resort with copy naming the *account*
   password explicitly.

9. **Chrome 142+ Local Network Access (PNA).** A public site fetching a
   loopback daemon fails with a bare network error (nothing in logs,
   nothing in the network panel's response) unless BOTH hold: the daemon's
   preflight answers `Access-Control-Allow-Private-Network: true` for the
   allowed origin (never `*`), and the fetch sets
   `targetAddressSpace: 'loopback'`. No header → Chrome just says
   "blocked". **Rule: public-site→loopback features need the header pair,
   and a PNA failure looks identical to a dead daemon — log the server
   side of the request before blaming the network.**

## Vault key handoff (`engram handoff`)

The box daemon derives the sync keys from `ENGRAM_PASSPHRASE` at startup;
they exist only in its memory. `engram handoff` mints a single-use 15-min
token against the local daemon and prints a link the user opens in the
signed-in browser:

```
engram handoff
  → POST daemon /sync/key-handoff/start → token (in-memory map, 900s TTL)
  → link: https://engram.ellmstack.dev/app/#/handoff/{token}?daemon=127.0.0.1:8787
  → SPA: account.get (signed-out → stash token, login, resume)
    → POST /sync/key-handoff/{token} (redeem FIRST — dead link fails fast)
    → A from memory → BroadcastChannel → password prompt (last resort)
    → wrap composite K (enc‖hmac‖vault_id) under A → PUT /account/vaults/{id}/wrap
  → vault is open-by-default; the vault passphrase was never typed or shown
```

- The token IS the credential; it redeems exactly once and expired tokens
  are swept on access. Tokens live in daemon memory — a daemon restart
  invalidates every outstanding link.
- Trust boundary: Caddy gates `/sync*` behind the box basic-auth, the same
  wall as the config routes. The keys never touch the relay.
- PNA pair (see pitfall 9) lives in the daemon's `pna_opt_in` middleware
  (`crates/engramd/src/main.rs`) and the SPA fetch
  (`targetAddressSpace`).
- The `?daemon=` param points at the user's loopback (or tunnel-forwarded)
  daemon — the CLI prints `127.0.0.1:8787` by default.

## Device linking (`engram link`)

One-click machine linking, WARP-style. `engram link` in a terminal →
browser opens → sign in → click "Link this machine" → the CLI receives an
account API key automatically. Pairing codes remain the headless/SSH path.

**Flow:** the CLI mints an ephemeral X25519 keypair and POSTs the public
key to `/devices/link-intents` (unauthenticated, 201 `{id, code,
relay_public_key}` — the code is shown once, stored as sha256 only). The
browser opens `{site}/#/link/{id}?code={code}`; the signed-in SPA POSTs
`/devices/link-intents/{id}/confirm` (Bearer session, hash-compared code).
The relay mints the usual unscoped `en_` key, seals it, and atomically
flips the intent pending→confirmed. The CLI polls `/status` every 2s;
the first poll claims the seal (confirmed→delivered, one-shot) and
decrypts it with its ephemeral private key.

**Seal format:** the relay's keypair is derived per intent —
`sk_r = X25519(SHA-256("engram-link-relay-v1" ‖ id ‖ code_hash))` — so no
private material is ever at rest and relay restarts don't kill live
intents. `shared = ECDH(sk_r, pk_cli)`, `key = SHA-256("engram-link-v1" ‖
shared)`, then ChaCha20-Poly1305 with a random 12-byte nonce and
AAD `"engram-link-v1" ‖ id`. Both sides reject an all-zeros shared secret
(low-order public keys). Binary fields are base64url; responses carry
`v: 1`.

**Threat model:** a leaked confirm URL is useless without the session AND
the code AND the CLI's private key — the seal it produces is an
undecryptable blob for anyone else. The code is single-use with a 10-minute
TTL; confirm requires a live session (401 otherwise, and the SPA stashes
id+code in sessionStorage to survive the sign-in round-trip, single-shot).
Intent endpoints are bucket-limited (link-create/confirm 5/s, link-status
20/s). The one residual trade-off: the delivered key sits in
`config.json` plaintext like every sync key today.

**Re-linking:** `engram link` bails when the vault already has a sync key;
`--force` replaces it, orphaning the old key row (revocable in
Account & Sync → API keys).

## Verification checklist for future auth work

- [ ] Preflight OPTIONS for EVERY method the SPA uses, with the real
      `Origin`, `Access-Control-Request-Method`, `-Headers`
- [ ] Full signup loop with a throwaway account incl. the two wrap PUTs
- [ ] Aborted-signup re-entry: signup → fail gate → signin → gate re-shows
- [ ] Kill-a-session test: revoke server-side → every screen reads signed-out
- [ ] Duplicate email → 409 `email_taken`, message tells the user to sign in
- [ ] Reset → all sessions revoked → recovery gate → rewrap → open vaults work

## File map

- Relay: `crates/engramd-sync/src/password_routes.rs` (signup/signin/reset),
  `account_routes.rs` (sessions, credentials, passkeys, wrap CRUD,
  `/account/vaults` `is_open`), CORS in `main.rs` (~line 580)
- Daemon: `crates/engramd/src/routes/key_handoff.rs` (mint/redeem),
  `crates/engramd/src/cli.rs` (`engram handoff`, `--site` default),
  PNA middleware in `crates/engramd/src/main.rs`
- SPA: `ui/engram-vault/js/main.js` — `route('/login')` (~line 763) owns
  the signin/signup/forgot/phrase/recovery views; `route('/handoff/:token')`
  (~line 1384) the handoff flow; `api.account.*` (~line 153); global chip
  + `onApi401` (~line 300); `ui/engram-vault/js/unlock.js` — Argon2id,
  AES-GCM wrap/unwrap, account key A state, `requestAccountKey`
  BroadcastChannel
- Deploy: `deploy/sync-relay.md` (relay runbook + deploy log),
  `deploy/caddy/sync.Caddyfile` (no CORS there — headers come from the app)
