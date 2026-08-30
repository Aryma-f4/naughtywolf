use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::crypto;

pub const PROTOCOL_VERSION: u8 = 1;

/// Top-level wire frame both sides speak. Serialized as JSON (structured,
/// length-framed by the transport); inner payload is AEAD-encrypted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u8,
    pub id: u64,
    pub session_id: Option<Uuid>,
    pub kind: Kind,
    /// base64(AEAD(inner_json, aad=envelope.id))
    pub encrypted: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Register,
    RegisterAck,
    Task,
    TaskResult,
    Heartbeat,
    Kill,
}

impl Envelope {
    pub fn new(kind: Kind, id: u64, session_id: Option<Uuid>, encrypted: String) -> Self {
        Envelope { version: PROTOCOL_VERSION, id, kind, session_id, encrypted }
    }

    /// Seal the whole frame (kind, id, session, payload) under one AEAD blob so
    /// no metadata is observable on the wire. Returns base64(nonce||ct).
    pub fn seal(&self, key: &[u8; crypto::KEY_LEN]) -> Result<String, EnvelopeError> {
        let pt = serde_json::to_vec(self).map_err(|e| EnvelopeError::Serde(e.to_string()))?;
        let blob = crypto::seal(key, &pt).map_err(EnvelopeError::Crypto)?;
        use base64::Engine;
        Ok(base64::engine::general_purpose::STANDARD.encode(blob))
    }

    /// Open a frame sealed by [`Envelope::seal`].
    pub fn open(key: &[u8; crypto::KEY_LEN], wire: &str) -> Result<Envelope, EnvelopeError> {
        use base64::Engine;
        let blob = base64::engine::general_purpose::STANDARD
            .decode(wire)
            .map_err(|e| EnvelopeError::Crypto(crypto::CryptoError::B64(e)))?;
        let pt = crypto::open(key, &blob).map_err(EnvelopeError::Crypto)?;
        serde_json::from_slice(&pt).map_err(|e| EnvelopeError::Serde(e.to_string()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EnvelopeError {
    #[error("crypto: {0}")]
    Crypto(#[from] crypto::CryptoError),
    #[error("serialize: {0}")]
    Serde(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_json_round_trip() {
        let e = Envelope::new(Kind::Heartbeat, 5, None, "abc".into());
        let j = serde_json::to_string(&e).unwrap();
        let back: Envelope = serde_json::from_str(&j).unwrap();
        assert_eq!(back.id, 5);
        assert_eq!(back.kind, Kind::Heartbeat);
        assert_eq!(back.encrypted, "abc");
    }

    #[test]
    fn seal_hides_frame_and_round_trips() {
        let key = [7u8; crypto::KEY_LEN];
        let e = Envelope::new(Kind::Register, 42, None, "secret".into());
        let sealed = e.seal(&key).unwrap();
        // The wire blob must not expose any structured (JSON) frame content.
        assert!(!sealed.contains("Register"));
        assert!(!sealed.contains("secret"));
        assert!(!sealed.contains("42"));
        let back = Envelope::open(&key, &sealed).unwrap();
        assert_eq!(back.id, e.id);
        assert_eq!(back.kind, e.kind);
        assert_eq!(back.encrypted, e.encrypted);
        // Wrong key must fail to open.
        assert!(Envelope::open(&[9u8; 32], &sealed).is_err());
    }
}
