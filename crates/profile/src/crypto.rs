use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use sha2::{Digest, Sha256};

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;

/// HKDF salt for deriving the per-session key from the x25519 shared secret.
pub const SESSION_SALT: &[u8] = b"nw-m2-session";

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("key exchange failed: {0}")]
    Exchange(String),
    #[error("encrypt failed: {0}")]
    Encrypt(String),
    #[error("decrypt failed: {0}")]
    Decrypt(String),
    #[error("base64: {0}")]
    B64(#[from] base64::DecodeError),
    #[error("utf8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

/// Derive a 32-byte session key from an x25519 shared secret using HKDF-SHA256.
pub fn derive_key(shared_secret: &[u8], salt: &[u8]) -> [u8; KEY_LEN] {
    let hk = hkdf::Hkdf::<Sha256>::new(Some(salt), shared_secret);
    let mut okm = [0u8; KEY_LEN];
    hk.expand(b"naughtywolf-session", &mut okm)
        .expect("key expansion of valid length cannot fail");
    okm
}

/// Deterministic 12-byte nonce from a counter + optional entropy; for AEAD the
/// nonce must never repeat under the same key, so we hash a per-envelope id.
pub fn nonce_for(envelope_id: u64) -> [u8; NONCE_LEN] {
    let mut h = Sha256::new();
    h.update(envelope_id.to_le_bytes());
    h.update(b"nw-nonce");
    let d = h.finalize();
    let mut n = [0u8; NONCE_LEN];
    n.copy_from_slice(&d[..NONCE_LEN]);
    n
}

/// Encrypt plaintext with AES-256-GCM. Returns `CIPHERTEXT||TAG` in base64.
pub fn encrypt(
    key: &[u8; KEY_LEN],
    envelope_id: u64,
    plaintext: &[u8],
) -> Result<String, CryptoError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| CryptoError::Encrypt(e.to_string()))?;
    let nonce_bytes = nonce_for(envelope_id);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad: &envelope_id.to_le_bytes(),
            },
        )
        .map_err(|e| CryptoError::Encrypt(e.to_string()))?;
    Ok(B64.encode(ct))
}

pub fn decrypt(
    key: &[u8; KEY_LEN],
    envelope_id: u64,
    ciphertext_b64: &str,
) -> Result<Vec<u8>, CryptoError> {
    let ct = B64.decode(ciphertext_b64)?;
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| CryptoError::Decrypt(e.to_string()))?;
    let nonce_bytes = nonce_for(envelope_id);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let pt = cipher
        .decrypt(
            nonce,
            Payload {
                msg: &ct,
                aad: &envelope_id.to_le_bytes(),
            },
        )
        .map_err(|e| CryptoError::Decrypt(e.to_string()))?;
    Ok(pt)
}

/// Authenticated seal with a fresh random nonce: returns `nonce || ct(tag)`.
/// Used for the outer envelope frame so per-wire metadata is not observable.
pub fn seal(key: &[u8; KEY_LEN], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let nonce_bytes: [u8; NONCE_LEN] = rand::random();
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| CryptoError::Encrypt(e.to_string()))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| CryptoError::Encrypt(e.to_string()))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Open a value sealed by [`seal`].
pub fn open(key: &[u8; KEY_LEN], data: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if data.len() < NONCE_LEN {
        return Err(CryptoError::Decrypt("sealed blob too short".into()));
    }
    let (nonce_bytes, ct) = data.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| CryptoError::Decrypt(e.to_string()))?;
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ct)
        .map_err(|e| CryptoError::Decrypt(e.to_string()))
}

/// Session key pair for x25519 key exchange. Holds static secret for handshake.
pub struct KeyPair {
    secret: x25519_dalek::StaticSecret,
    public: x25519_dalek::PublicKey,
}

impl KeyPair {
    pub fn generate() -> Self {
        let secret = x25519_dalek::StaticSecret::random_from_rng(rand::thread_rng());
        let public = x25519_dalek::PublicKey::from(&secret);
        KeyPair { secret, public }
    }

    /// HPKE-ish static-static: public_bytes is the peer's x25519 public key.
    pub fn shared_secret(&self, peer_public: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let pk = x25519_dalek::PublicKey::from(
            <[u8; 32]>::try_from(peer_public)
                .map_err(|_| CryptoError::Exchange("peer public key must be 32 bytes".into()))?,
        );
        Ok(self.secret.diffie_hellman(&pk).to_bytes().to_vec())
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.public.to_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_key_is_stable() {
        let k1 = derive_key(b"shared", b"salt");
        let k2 = derive_key(b"shared", b"salt");
        assert_eq!(k1, k2);
        let k3 = derive_key(b"shared", b"other");
        assert_ne!(k1, k3);
    }

    #[test]
    fn key_exchange_produces_shared_secret_both_sides() {
        let a = KeyPair::generate();
        let b = KeyPair::generate();
        let sa = a.shared_secret(&b.public_key()).unwrap();
        let sb = b.shared_secret(&a.public_key()).unwrap();
        assert_eq!(sa, sb);
        assert_eq!(sa.len(), 32);
    }

    #[test]
    fn encrypt_round_trips() {
        let key = derive_key(&[7u8; 32], b"s");
        let ct = encrypt(&key, 99, b"secret payload").unwrap();
        assert_ne!(ct, "secret payload");
        let pt = decrypt(&key, 99, &ct).unwrap();
        assert_eq!(pt, b"secret payload");
    }

    #[test]
    fn decrypt_rejects_wrong_id() {
        let key = derive_key(&[7u8; 32], b"s");
        let ct = encrypt(&key, 1, b"data").unwrap();
        assert!(decrypt(&key, 2, &ct).is_err());
    }

    #[test]
    fn nonce_varies_by_id() {
        assert_ne!(nonce_for(1), nonce_for(2));
    }
}
