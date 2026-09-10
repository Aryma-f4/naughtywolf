use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

use crate::crypto;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GSocketConfig {
    pub secret: String,
    pub local_port: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PayloadConfig {
    pub endpoint: String,
    pub psk: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gsocket: Option<GSocketConfig>,
}

/// Fixed obfuscation mask for the per-build key so the key is not stored as a
/// contiguous plaintext blob in the binary. This only defeats static string
/// search (the chosen anti-RE level), not a determined in-memory reversal.
const PEPPER: &[u8] = b"nw-cfg-pepper-9f3";

/// Bake `endpoint` + `psk` into a single base64 blob using AES-GCM with a
/// fresh per-build key. The key travels in the blob masked by `PEPPER`.
/// Layout (pre-base64): `[1B key_len][key XOR pepper][GCM ciphertext]`.
pub fn encrypt_config(endpoint: &str, psk: &str) -> Result<String, crypto::CryptoError> {
    encrypt_value(&serde_json::json!({ "endpoint": endpoint, "psk": psk }))
}

pub fn encrypt_payload_config(config: &PayloadConfig) -> Result<String, crypto::CryptoError> {
    encrypt_value(&serde_json::json!({ "version": 2, "config": config }))
}

fn encrypt_value(pt: &serde_json::Value) -> Result<String, crypto::CryptoError> {
    let key = random_key();
    let json = serde_json::to_vec(&pt).map_err(|e| crypto::CryptoError::Encrypt(e.to_string()))?;
    // GCM appends a 16-byte tag, so this already authenticates the config.
    let ciphertext_b64 = crypto::encrypt(&key, 0, &json)?;
    let ct = B64.decode(&ciphertext_b64)?;

    let mut raw = Vec::with_capacity(1 + key.len() + ct.len());
    raw.push(key.len() as u8);
    raw.extend(xor(&key, PEPPER));
    raw.extend(ct);
    Ok(B64.encode(raw))
}

/// Recover `(endpoint, psk)` from a blob produced by `encrypt_config`.
pub fn decrypt_config(blob: &str) -> Result<(String, String), crypto::CryptoError> {
    let config = decrypt_payload_config(blob)?;
    Ok((config.endpoint, config.psk))
}

pub fn decrypt_payload_config(blob: &str) -> Result<PayloadConfig, crypto::CryptoError> {
    let v = decrypt_value(blob)?;
    if v.get("version").and_then(|value| value.as_u64()) == Some(2) {
        return serde_json::from_value(
            v.get("config")
                .cloned()
                .ok_or_else(|| crypto::CryptoError::Decrypt("missing config".into()))?,
        )
        .map_err(|e| crypto::CryptoError::Decrypt(e.to_string()));
    }
    let endpoint = v
        .get("endpoint")
        .and_then(|x| x.as_str())
        .ok_or_else(|| crypto::CryptoError::Decrypt("missing endpoint".into()))?
        .to_owned();
    let psk = v
        .get("psk")
        .and_then(|x| x.as_str())
        .ok_or_else(|| crypto::CryptoError::Decrypt("missing psk".into()))?
        .to_owned();
    Ok(PayloadConfig {
        endpoint,
        psk,
        gsocket: None,
    })
}

fn decrypt_value(blob: &str) -> Result<serde_json::Value, crypto::CryptoError> {
    let raw = B64.decode(blob)?;
    if raw.len() < 2 {
        return Err(crypto::CryptoError::Decrypt("config blob too short".into()));
    }
    let key_len = raw[0] as usize;
    if key_len == 0 || key_len + 1 > raw.len() {
        return Err(crypto::CryptoError::Decrypt("bad config key length".into()));
    }
    let mut key = [0u8; crypto::KEY_LEN];
    let masked = &raw[1..1 + key_len];
    if masked.len() > key.len() {
        return Err(crypto::CryptoError::Decrypt("config key too long".into()));
    }
    key[..masked.len()].copy_from_slice(&xor(masked, PEPPER));
    let ct = B64.encode(&raw[1 + key_len..]);
    let json = crypto::decrypt(&key, 0, &ct)?;
    serde_json::from_slice(&json).map_err(|e| crypto::CryptoError::Decrypt(e.to_string()))
}

fn random_key() -> [u8; crypto::KEY_LEN] {
    use rand::RngCore;
    let mut k = [0u8; crypto::KEY_LEN];
    rand::thread_rng().fill_bytes(&mut k);
    k
}

fn xor(a: &[u8], b: &[u8]) -> Vec<u8> {
    a.iter().zip(b.iter().cycle()).map(|(x, y)| x ^ y).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gsocket_config_round_trips_without_plaintext_secrets() {
        let config = PayloadConfig {
            endpoint: "gs://127.0.0.1:4630".into(),
            psk: "nw-secret".into(),
            gsocket: Some(GSocketConfig {
                secret: "gs-secret".into(),
                local_port: 4630,
            }),
        };

        let blob = encrypt_payload_config(&config).unwrap();
        assert!(!blob.contains("nw-secret"));
        assert!(!blob.contains("gs-secret"));
        assert_eq!(decrypt_payload_config(&blob).unwrap(), config);
    }

    #[test]
    fn legacy_config_decodes_as_payload_config() {
        let blob = encrypt_config("tcp://10.0.0.5:4444", "legacy-psk").unwrap();
        assert_eq!(
            decrypt_payload_config(&blob).unwrap(),
            PayloadConfig {
                endpoint: "tcp://10.0.0.5:4444".into(),
                psk: "legacy-psk".into(),
                gsocket: None,
            }
        );
    }

    #[test]
    fn config_round_trips() {
        let blob = encrypt_config("tcp://10.0.0.5:4444", "s3cret-psk").unwrap();
        assert!(!blob.contains("10.0.0.5"));
        assert!(!blob.contains("s3cret"));
        let (endpoint, psk) = decrypt_config(&blob).unwrap();
        assert_eq!(endpoint, "tcp://10.0.0.5:4444");
        assert_eq!(psk, "s3cret-psk");
    }

    #[test]
    fn different_builds_produce_different_blobs() {
        let a = encrypt_config("http://a:1", "p").unwrap();
        let b = encrypt_config("http://a:1", "p").unwrap();
        assert_ne!(a, b);
        assert_eq!(decrypt_config(&a).unwrap(), decrypt_config(&b).unwrap());
    }

    #[test]
    fn garbage_blob_is_rejected() {
        assert!(decrypt_config("!!not-base64!!").is_err());
        assert!(decrypt_config(&B64.encode(b"junk")).is_err());
    }
}
