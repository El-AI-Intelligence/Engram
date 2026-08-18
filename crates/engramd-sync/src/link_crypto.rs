//! X25519 + ChaCha20-Poly1305 helpers for `engram link` — one-click
//! machine linking (WARP-style onboarding).
//!
//! The CLI posts its ephemeral X25519 public key; the relay derives its
//! own per-intent keypair deterministically from (id, code_hash) so no
//! private material is ever at rest (code_hash is already stored as a
//! sha256, matching the relay's secret discipline). Both sides compute the
//! same shared secret and the freshly minted account API key is sealed to
//! it — a leaked confirm URL yields only an undecryptable blob.

use anyhow::{anyhow, Result};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::TryRngCore;
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

/// KDF label for the link seal: SHA-256("engram-link-v1" ‖ shared_secret).
pub const LINK_KDF_LABEL: &[u8] = b"engram-link-v1";
/// Derivation label for the relay's per-intent keypair:
/// SHA-256("engram-link-relay-v1" ‖ id ‖ code_hash) → StaticSecret seed.
pub const RELAY_KEY_LABEL: &[u8] = b"engram-link-relay-v1";

/// Derive the relay's per-intent X25519 keypair from (id, code_hash).
/// Deterministic: a relay restart re-derives the same key, so live intents
/// survive restarts; the secret is only ever materialized in memory.
pub fn intent_keypair(id: &str, code_hash: &[u8; 32]) -> StaticSecret {
    let mut hasher = Sha256::new();
    hasher.update(RELAY_KEY_LABEL);
    hasher.update(id.as_bytes());
    hasher.update(code_hash);
    let seed: [u8; 32] = hasher.finalize().into();
    StaticSecret::from(seed)
}

/// ECDH shared secret between our (derived) secret and the CLI's public key.
pub fn link_shared_secret(sk: &StaticSecret, cli_pk: &PublicKey) -> Result<[u8; 32]> {
    let shared = sk.diffie_hellman(cli_pk);
    let bytes = *shared.as_bytes();
    // Reject all-zeros: an attacker-supplied low-order/identity public key
    // would otherwise produce a predictable seal key.
    if bytes.iter().all(|&b| b == 0) {
        return Err(anyhow!("degenerate X25519 shared secret (all zeros)"));
    }
    Ok(bytes)
}

/// KDF: SHA-256("engram-link-v1" ‖ shared) → 32-byte ChaCha key.
pub fn link_kdf(shared: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(LINK_KDF_LABEL);
    hasher.update(shared);
    hasher.finalize().into()
}

/// AAD binds the ciphertext to its intent: "engram-link-v1" ‖ id — a
/// claimed seal can never be replayed against a different intent.
fn link_aad(id: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity(LINK_KDF_LABEL.len() + id.len());
    aad.extend_from_slice(LINK_KDF_LABEL);
    aad.extend_from_slice(id.as_bytes());
    aad
}

/// Seal an API key to the shared secret: ChaCha20-Poly1305, random
/// 12-byte nonce. Returns (ciphertext, nonce).
pub fn seal_api_key(id: &str, shared: &[u8], api_key: &str) -> Result<(Vec<u8>, [u8; 12])> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&link_kdf(shared)));
    let mut nonce_bytes = [0u8; 12];
    rand::rngs::OsRng
        .try_fill_bytes(&mut nonce_bytes)
        .map_err(|e| anyhow!("OS RNG failure: {e}"))?;
    let ct = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: api_key.as_bytes(),
                aad: &link_aad(id),
            },
        )
        .map_err(|_| anyhow!("seal failed"))?;
    Ok((ct, nonce_bytes))
}

/// Mirror of the CLI's decrypt — used by relay-side tests to prove the
/// seal opens from the CLI's side of the handshake.
pub fn unseal_api_key(id: &str, shared: &[u8], ct: &[u8], nonce: &[u8]) -> Result<String> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&link_kdf(shared)));
    let pt = cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ct,
                aad: &link_aad(id),
            },
        )
        .map_err(|_| anyhow!("unseal failed (wrong key, tampered ciphertext, or wrong intent)"))?;
    String::from_utf8(pt).map_err(|_| anyhow!("decrypted key is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_cli_keypair() -> (StaticSecret, PublicKey) {
        let sk = StaticSecret::from([7u8; 32]);
        let pk = PublicKey::from(&sk);
        (sk, pk)
    }

    #[test]
    fn intent_keypair_is_deterministic() {
        let code_hash = crate::auth::hash_key("ENG-ABCD-EFGH-JKLM");
        let a = intent_keypair("intent-1", &code_hash);
        let b = intent_keypair("intent-1", &code_hash);
        assert_eq!(PublicKey::from(&a).as_bytes(), PublicKey::from(&b).as_bytes());
        // Different id or different code → different key.
        let c = intent_keypair("intent-2", &code_hash);
        assert_ne!(PublicKey::from(&a).as_bytes(), PublicKey::from(&c).as_bytes());
    }

    #[test]
    fn kdf_is_deterministic() {
        assert_eq!(link_kdf(b"shared-a"), link_kdf(b"shared-a"));
        assert_ne!(link_kdf(b"shared-a"), link_kdf(b"shared-b"));
    }

    #[test]
    fn zero_shared_secret_is_rejected() {
        // All-zeros public key → all-zeros shared secret.
        let sk = StaticSecret::from([9u8; 32]);
        let zero_pk = PublicKey::from([0u8; 32]);
        assert!(link_shared_secret(&sk, &zero_pk).is_err());
    }

    #[test]
    fn seal_round_trip() {
        let (sk_cli, pk_cli) = fake_cli_keypair();
        let code_hash = crate::auth::hash_key("ENG-ABCD-EFGH-JKLM");
        let id = "intent-roundtrip";
        let sk_r = intent_keypair(id, &code_hash);
        let shared = link_shared_secret(&sk_r, &pk_cli).unwrap();
        let (ct, nonce) = seal_api_key(id, &shared, "en_test_key_123").unwrap();
        let opened = unseal_api_key(id, &shared, &ct, &nonce).unwrap();
        assert_eq!(opened, "en_test_key_123");
        // The CLI computes the same shared secret from its side.
        let shared_cli = link_shared_secret(&sk_cli, &PublicKey::from(&sk_r)).unwrap();
        assert_eq!(shared, shared_cli);
    }

    #[test]
    fn wrong_intent_aad_fails() {
        let (_, pk_cli) = fake_cli_keypair();
        let code_hash = crate::auth::hash_key("ENG-ABCD-EFGH-JKLM");
        let sk_r = intent_keypair("intent-1", &code_hash);
        let shared = link_shared_secret(&sk_r, &pk_cli).unwrap();
        let (ct, nonce) = seal_api_key("intent-1", &shared, "en_test_key").unwrap();
        assert!(unseal_api_key("intent-2", &shared, &ct, &nonce).is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let (_, pk_cli) = fake_cli_keypair();
        let code_hash = crate::auth::hash_key("ENG-ABCD-EFGH-JKLM");
        let sk_r = intent_keypair("intent-1", &code_hash);
        let shared = link_shared_secret(&sk_r, &pk_cli).unwrap();
        let (mut ct, nonce) = seal_api_key("intent-1", &shared, "en_test_key").unwrap();
        ct[0] ^= 0x01;
        assert!(unseal_api_key("intent-1", &shared, &ct, &nonce).is_err());
    }
}
