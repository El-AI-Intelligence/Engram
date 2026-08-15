//! WebAuthn + account auth primitives for the relay.
//!
//! Accounts are standalone passkeys: registration is the whole "sign up"
//! story, login is the same ceremony with a discoverable credential, and
//! the relay never stores an email, name, or any other PII — only
//! credential public material and hashed secrets. Billing (and the PII it
//! needs) is a separate private service (roadmap 1.3); it keys accounts
//! by the opaque account id.
//!
//! Secrets at rest are always hashed:
//!   - API keys    → sha256(full key) in `api_keys.key_hash`; the plaintext
//!     is shown exactly once at creation.
//!   - Session     → sha256(token) in `sessions.token_hash`; the Bearer
//!     token lives only in the browser's localStorage.

use base64::Engine;
use rand::TryRngCore;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use webauthn_rs::prelude::*;

pub use webauthn_rs::Webauthn;

/// Session lifetime: tokens expire 7 days after minting.
pub const SESSION_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// Pending-ceremony lifetime: registration/authentication state that is
/// not finished within this window is dropped (and a "we never saw this
/// challenge" error returns instead of a confusing mismatch).
pub const CEREMONY_TTL: Duration = Duration::from_secs(300);

/// Account API keys are self-describing: `en_` + 32 random bytes
/// (base64url). Served keys are 43 chars, comfortably above the 16-char
/// floor for the legacy static-key parser.
pub const API_KEY_PREFIX: &str = "en_";

/// sha256 of arbitrary bytes — the one-way step before storing or looking
/// up any secret.
pub fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// sha256 of an API key (used for at-rest storage and hash-indexed lookup).
pub fn hash_key(key: &str) -> [u8; 32] {
    hash_bytes(key.as_bytes())
}

/// sha256 of a session token.
pub fn hash_token(token: &str) -> [u8; 32] {
    hash_bytes(token.as_bytes())
}

/// Stable base64url (no padding) of a key hash — used as the rate-limiter
/// map key so plaintext keys never live in limiter state.
pub fn hash_b64(hash: &[u8; 32]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash)
}

fn random_base64url(n_bytes: usize) -> anyhow::Result<String> {
    let mut buf = vec![0u8; n_bytes];
    rand::rngs::OsRng
        .try_fill_bytes(&mut buf)
        .map_err(|e| anyhow::anyhow!("OS RNG failure: {e}"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}

/// Mint a fresh account API key: `en_` + 32 random bytes (base64url).
pub fn generate_api_key() -> anyhow::Result<String> {
    Ok(format!("{API_KEY_PREFIX}{}", random_base64url(32)?))
}

/// Mint a fresh session token: 32 random bytes (base64url). Only its
/// sha256 is stored server-side.
pub fn mint_session_token() -> anyhow::Result<String> {
    random_base64url(32)
}

/// In-memory state for in-flight WebAuthn ceremonies.
///
/// webauthn-rs ceremonies are two requests (start → finish), so the server
/// must remember the challenge state between them. Like Guardrail's store,
/// this is an in-memory map with a short TTL — fine for a single-instance
/// relay (the managed relay is one process; a HA deployment would need
/// shared state, documented in SYNC.md).
pub struct WebauthnStore {
    /// registration id → (challenge, state, account to attach to, started).
    /// `account` is Some when the caller already has a session (adding a
    /// passkey to an existing account); None = this ceremony creates the
    /// account.
    registrations: Mutex<HashMap<String, (CreationChallengeResponse, PasskeyRegistration, Option<String>, Instant)>>,
    authentications: Mutex<HashMap<String, (RequestChallengeResponse, PasskeyAuthentication, Instant)>>,
}

impl WebauthnStore {
    pub fn new() -> Self {
        Self {
            registrations: Mutex::new(HashMap::new()),
            authentications: Mutex::new(HashMap::new()),
        }
    }

    /// Store the server-side half of a registration ceremony.
    pub fn put_registration(
        &self,
        id: String,
        challenge: CreationChallengeResponse,
        state: PasskeyRegistration,
        account: Option<String>,
    ) {
        let mut map = self.registrations.lock().unwrap();
        let deadline = Instant::now() - CEREMONY_TTL;
        map.retain(|_, (_, _, _, started)| *started > deadline);
        map.insert(id, (challenge, state, account, Instant::now()));
    }

    /// Take (and consume) a registration ceremony. `now` is injectable so
    /// TTL expiry is testable without sleeping.
    pub fn take_registration(
        &self,
        id: &str,
        now: Instant,
    ) -> Option<(CreationChallengeResponse, PasskeyRegistration, Option<String>)> {
        let mut map = self.registrations.lock().unwrap();
        let entry = map.remove(id)?;
        if now.duration_since(entry.3) >= CEREMONY_TTL {
            return None;
        }
        Some((entry.0, entry.1, entry.2))
    }

    /// Store the server-side half of an authentication ceremony.
    pub fn put_authentication(
        &self,
        id: String,
        challenge: RequestChallengeResponse,
        state: PasskeyAuthentication,
    ) {
        let mut map = self.authentications.lock().unwrap();
        let deadline = Instant::now() - CEREMONY_TTL;
        map.retain(|_, (_, _, started)| *started > deadline);
        map.insert(id, (challenge, state, Instant::now()));
    }

    /// Take (and consume) an authentication ceremony.
    pub fn take_authentication(
        &self,
        id: &str,
        now: Instant,
    ) -> Option<(RequestChallengeResponse, PasskeyAuthentication)> {
        let mut map = self.authentications.lock().unwrap();
        let entry = map.remove(id)?;
        if now.duration_since(entry.2) >= CEREMONY_TTL {
            return None;
        }
        Some((entry.0, entry.1))
    }
}

impl Default for WebauthnStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the WebAuthn instance for this server.
///
/// `rp_id` must be a registrable domain suffix of the browser origin
/// (the vault UI's `window.location.origin`) — passkeys bind to it, so
/// changing it later orphans every existing passkey. `origin` is the
/// primary allowed origin; the full allow-list lives in `SyncState` and
/// each finish call validates the client's origin against it.
pub fn build_webauthn(rp_id: &str, origin: &str) -> anyhow::Result<Webauthn> {
    let origin_url = Url::parse(origin)
        .map_err(|e| anyhow::anyhow!("invalid --origin {origin:?}: {e}"))?;
    WebauthnBuilder::new(rp_id, &origin_url)
        .map_err(|e| anyhow::anyhow!("webauthn setup failed for rp_id {rp_id:?}: {e}"))?
        .rp_name("Engram Sync")
        .build()
        .map_err(|e| anyhow::anyhow!("webauthn build failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_hashes_are_stable_and_distinct() {
        let a = hash_key("some-key-value-0001");
        assert_eq!(a, hash_key("some-key-value-0001"));
        assert_ne!(a, hash_key("some-key-value-0002"));
        assert_eq!(a.len(), 32);
        // hash_b64 round-trips the same hash to the same limiter key
        assert_eq!(hash_b64(&a), hash_b64(&hash_key("some-key-value-0001")));
    }

    #[test]
    fn generated_keys_have_prefix_and_entropy() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..100 {
            let key = generate_api_key().unwrap();
            assert!(key.starts_with(API_KEY_PREFIX));
            assert!(key.len() >= 43, "32 random bytes → 43 base64url chars + prefix");
            assert!(seen.insert(key.clone()), "duplicate API key generated");
        }
    }

    #[test]
    fn session_tokens_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..100 {
            let token = mint_session_token().unwrap();
            assert_eq!(token.len(), 43);
            assert!(seen.insert(token), "duplicate session token generated");
        }
    }

    #[test]
    fn registration_store_round_trips_and_expires() {
        let store = WebauthnStore::new();
        // Build a plausible challenge state without a browser: we only test
        // the store mechanics, so the values can be minimal.
        let webauthn = build_webauthn("localhost", "http://localhost:8787").unwrap();
        let (challenge, state) = webauthn
            .start_passkey_registration(
                Uuid::new_v4(),
                "test",
                "test",
                None,
            )
            .unwrap();
        store.put_registration("ch1".into(), challenge.clone(), state.clone(), None);

        let now = Instant::now();
        let taken = store.take_registration("ch1", now);
        assert!(taken.is_some(), "fresh ceremony must be retrievable");
        assert!(taken.unwrap().2.is_none(), "no attach account in this test");
        // Consumed: second take returns None
        assert!(store.take_registration("ch1", now).is_none());
        // Expired: TTL check rejects
        store.put_registration("ch2".into(), challenge, state, Some("account-1".into()));
        let later = now + CEREMONY_TTL + Duration::from_secs(1);
        assert!(
            store.take_registration("ch2", later).is_none(),
            "expired ceremony must be rejected"
        );
    }
}
