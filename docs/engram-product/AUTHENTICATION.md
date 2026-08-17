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
- SPA: `ui/engram-vault/js/main.js` — `route('/login')` (~line 763) owns
  the signin/signup/forgot/phrase/recovery views; `api.account.*` (~line
  153); global chip + `onApi401` (~line 300); `ui/engram-vault/js/unlock.js`
  — Argon2id, AES-GCM wrap/unwrap, account key A state
- Deploy: `deploy/sync-relay.md` (relay runbook + deploy log),
  `deploy/caddy/sync.Caddyfile` (no CORS there — headers come from the app)
