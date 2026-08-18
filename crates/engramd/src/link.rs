//! Client side of `engram link` — the one-click machine linking flow.
//!
//! We mint an ephemeral X25519 keypair, post the public key to the relay,
//! and open the confirm URL in the browser. The signed-in browser approves,
//! the relay seals a freshly minted account API key to our public key, and
//! we poll for the sealed blob and decrypt it exactly once.
//!
//! Labels/KDF/AAD MUST match `engramd-sync/src/link_crypto.rs` — the
//! interop round-trip test below re-derives the relay's keypair from the
//! same labels and would break loudly if either side drifted.

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde_json::Value;
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

/// KDF label — identical to `LINK_KDF_LABEL` in engramd-sync/link_crypto.rs.
pub const LINK_KDF_LABEL: &[u8] = b"engram-link-v1";
/// Relay keypair derivation label — identical to `RELAY_KEY_LABEL` in
/// engramd-sync/link_crypto.rs.
pub const RELAY_KEY_LABEL: &[u8] = b"engram-link-relay-v1";

/// Fresh ephemeral keypair. The secret key exists only for the life of this
/// `engram link` run; the account key it protects is decrypted in memory
/// and stored by the caller (config.json), never the secret itself.
pub fn generate_ephemeral_keypair() -> (StaticSecret, PublicKey) {
    let sk = StaticSecret::random_from_rng(rand_core::OsRng);
    let pk = PublicKey::from(&sk);
    (sk, pk)
}

/// ECDH with the relay's public key; reject all-zeros (low-order relay key).
pub fn link_shared_secret(sk: &StaticSecret, relay_pk: &PublicKey) -> Result<[u8; 32]> {
    let shared = sk.diffie_hellman(relay_pk);
    let bytes = *shared.as_bytes();
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

/// AAD: "engram-link-v1" ‖ id — the seal only opens for this intent.
fn link_aad(id: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity(LINK_KDF_LABEL.len() + id.len());
    aad.extend_from_slice(LINK_KDF_LABEL);
    aad.extend_from_slice(id.as_bytes());
    aad
}

/// Decrypt the sealed account key delivered by the relay. Returns the
/// plaintext `en_…` API key; fails on tamper, wrong intent, or a
/// mismatched relay key (which would mean the response is not from the
/// relay we posted our key to).
pub fn decrypt_link_key(
    id: &str,
    relay_pk: &PublicKey,
    sk_cli: &StaticSecret,
    sealed: &[u8],
    nonce: &[u8],
) -> Result<String> {
    let shared = link_shared_secret(sk_cli, relay_pk)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&link_kdf(&shared)));
    let pt = cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: sealed,
                aad: &link_aad(id),
            },
        )
        .map_err(|_| anyhow!("could not decrypt the linked key — run `engram link` again"))?;
    String::from_utf8(pt).map_err(|_| anyhow!("decrypted key is not UTF-8"))
}

/// What a status poll returned, already shape-checked.
pub enum LinkStatus {
    /// The browser hasn't approved yet (or hasn't signed in yet).
    Pending,
    /// Confirmed — exactly once the sealed key is present. Decode with
    /// decrypt_link_key; a second poll will 410.
    Confirmed { sealed_key: Vec<u8>, nonce: Vec<u8> },
}

/// Parse a status-body into LinkStatus. 410/404 are the caller's concern
/// (poll exits with guidance); this is only for successful 200 bodies.
pub fn parse_link_status(body: &Value) -> Result<LinkStatus> {
    match body.get("status").and_then(|v| v.as_str()) {
        Some("pending") => Ok(LinkStatus::Pending),
        Some("confirmed") => {
            let sealed = body
                .get("sealed_key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("relay response missing sealed_key"))?;
            let nonce = body
                .get("nonce")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("relay response missing nonce"))?;
            let sealed_key = URL_SAFE_NO_PAD
                .decode(sealed.as_bytes())
                .map_err(|_| anyhow!("sealed_key is not base64url"))?;
            let nonce = URL_SAFE_NO_PAD
                .decode(nonce.as_bytes())
                .map_err(|_| anyhow!("nonce is not base64url"))?;
            Ok(LinkStatus::Confirmed { sealed_key, nonce })
        }
        _ => Err(anyhow!("unrecognized link status")),
    }
}

/// Open the confirm URL in the default browser. Best-effort — when the
/// platform has no launcher (SSH, containers) we just print the URL.
pub fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("open").arg(url).status();
    #[cfg(target_os = "windows")]
    let cmd = std::process::Command::new("cmd")
        .args(["/C", "start", "", url]) // empty title arg: start treats the URL as a URL, not a title
        .status();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let cmd = std::process::Command::new("xdg-open").arg(url).status();
    // Success or failure: the user can always open the printed URL by hand.
    let _ = cmd;
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::RngCore;

    /// Simulated relay: re-derives the relay keypair with the SAME labels as
    /// engramd-sync/src/link_crypto.rs (the interop lock-in) and seals a key.
    fn simulated_relay(id: &str, code: &str, cli_pk: &PublicKey, api_key: &str) -> (PublicKey, Vec<u8>, Vec<u8>) {
        let mut hasher = Sha256::new();
        hasher.update(RELAY_KEY_LABEL);
        hasher.update(id.as_bytes());
        let mut code_hasher = Sha256::new();
        code_hasher.update(code.as_bytes());
        let code_hash: [u8; 32] = code_hasher.finalize().into();
        hasher.update(code_hash);
        let seed: [u8; 32] = hasher.finalize().into();
        let sk_r = StaticSecret::from(seed);
        let pk_r = PublicKey::from(&sk_r);
        let shared = link_shared_secret(&sk_r, cli_pk).unwrap();
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&link_kdf(&shared)));
        let mut nonce_bytes = [0u8; 12];
        rand_core::OsRng.fill_bytes(&mut nonce_bytes);
        let sealed = cipher
            .encrypt(
                Nonce::from_slice(&nonce_bytes),
                Payload {
                    msg: api_key.as_bytes(),
                    aad: &link_aad(id),
                },
            )
            .unwrap();
        (pk_r, sealed, nonce_bytes.to_vec())
    }

    #[test]
    fn round_trip_with_simulated_relay() {
        let (sk_cli, pk_cli) = generate_ephemeral_keypair();
        let (pk_r, sealed, nonce) =
            simulated_relay("intent-1", "ENG-ABCD-EFGH-JKLM", &pk_cli, "en_fresh_account_key");
        let opened = decrypt_link_key("intent-1", &pk_r, &sk_cli, &sealed, &nonce).unwrap();
        assert_eq!(opened, "en_fresh_account_key");
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let (sk_cli, pk_cli) = generate_ephemeral_keypair();
        let (pk_r, mut sealed, nonce) =
            simulated_relay("intent-1", "ENG-ABCD-EFGH-JKLM", &pk_cli, "en_key");
        sealed[0] ^= 0x01;
        assert!(decrypt_link_key("intent-1", &pk_r, &sk_cli, &sealed, &nonce).is_err());
    }

    #[test]
    fn wrong_intent_aad_fails() {
        let (sk_cli, pk_cli) = generate_ephemeral_keypair();
        let (pk_r, sealed, nonce) =
            simulated_relay("intent-1", "ENG-ABCD-EFGH-JKLM", &pk_cli, "en_key");
        assert!(decrypt_link_key("intent-2", &pk_r, &sk_cli, &sealed, &nonce).is_err());
    }

    #[test]
    fn zero_relay_key_rejected() {
        let (sk_cli, _) = generate_ephemeral_keypair();
        let zero_pk = PublicKey::from([0u8; 32]);
        assert!(link_shared_secret(&sk_cli, &zero_pk).is_err());
    }

    #[test]
    fn parse_status_shapes() {
        let pending = parse_link_status(&serde_json::json!({"status": "pending", "v": 1})).unwrap();
        assert!(matches!(pending, LinkStatus::Pending));
        let sealed_b64 = URL_SAFE_NO_PAD.encode(b"sealed-bytes");
        let nonce_b64 = URL_SAFE_NO_PAD.encode(b"0123456789ab");
        let confirmed = parse_link_status(&serde_json::json!({
            "status": "confirmed", "sealed_key": sealed_b64, "nonce": nonce_b64, "v": 1
        }))
        .unwrap();
        match confirmed {
            LinkStatus::Confirmed { sealed_key, nonce } => {
                assert_eq!(sealed_key, b"sealed-bytes");
                assert_eq!(nonce, b"0123456789ab");
            }
            LinkStatus::Pending => panic!("expected confirmed"),
        }
        assert!(parse_link_status(&serde_json::json!({"status": "confirmed"})).is_err());
    }
}
