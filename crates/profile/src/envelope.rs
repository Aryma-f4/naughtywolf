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

/// Inner fields sealed under AEAD; `session_id` is carried in the clear as a
/// routing token so a single checkin endpoint can route each frame to the
/// right per-session key before opening it.
#[derive(Serialize, Deserialize)]
struct Core {
    version: u8,
    id: u64,
    kind: Kind,
    encrypted: String,
}

/// The transport wire frame: routing id in the clear, sealed core blob.
#[derive(Serialize, Deserialize)]
struct WireFrame {
    session_id: Option<Uuid>,
    blob: String,
}

fn b64(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

impl Envelope {
    pub fn new(kind: Kind, id: u64, session_id: Option<Uuid>, encrypted: String) -> Self {
        Envelope {
            version: PROTOCOL_VERSION,
            id,
            kind,
            session_id,
            encrypted,
        }
    }

    /// Seal the envelope: encrypt the inner core (kind/id/payload) under one
    /// AEAD blob, keeping only the routing `session_id` readable so the server
    /// can pick the per-session key before opening. Returns base64 of the wire
    /// frame `{ session_id, blob=base64(nonce||ct) }`.
    pub fn seal(&self, key: &[u8; crypto::KEY_LEN]) -> Result<String, EnvelopeError> {
        let core = Core {
            version: self.version,
            id: self.id,
            kind: self.kind,
            encrypted: self.encrypted.clone(),
        };
        let core_json =
            serde_json::to_vec(&core).map_err(|e| EnvelopeError::Serde(e.to_string()))?;
        let blob = crypto::seal(key, &core_json).map_err(EnvelopeError::Crypto)?;
        let wf = WireFrame {
            session_id: self.session_id,
            blob: b64(&blob),
        };
        let j = serde_json::to_vec(&wf).map_err(|e| EnvelopeError::Serde(e.to_string()))?;
        Ok(b64(&j))
    }

    /// Open a frame sealed by [`Envelope::seal`] with the given key, restoring
    /// the full envelope (routing session id included).
    pub fn open(key: &[u8; crypto::KEY_LEN], wire: &str) -> Result<Envelope, EnvelopeError> {
        let raw = b64_decode(wire).map_err(EnvelopeError::Crypto)?;
        let wf: WireFrame =
            serde_json::from_slice(&raw).map_err(|e| EnvelopeError::Serde(e.to_string()))?;
        let blob = b64_decode(&wf.blob).map_err(EnvelopeError::Crypto)?;
        let pt = crypto::open(key, &blob).map_err(EnvelopeError::Crypto)?;
        let core: Core =
            serde_json::from_slice(&pt).map_err(|e| EnvelopeError::Serde(e.to_string()))?;
        Ok(Envelope {
            version: core.version,
            id: core.id,
            kind: core.kind,
            encrypted: core.encrypted,
            session_id: wf.session_id,
        })
    }

    /// Read the clear routing session id from a sealed wire without opening the
    /// AEAD core. `None` signals a pre-registration (register) frame.
    pub fn routing_id(wire: &str) -> Option<Uuid> {
        let wf: WireFrame = serde_json::from_slice(&b64_decode(wire).ok()?).ok()?;
        wf.session_id
    }
}

fn b64_decode(s: &str) -> Result<Vec<u8>, crypto::CryptoError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(crypto::CryptoError::B64)
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
