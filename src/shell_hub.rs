//! The portal's view of shells attached to callbacks.
//!
//! An attached shell is live state, not a task: it exists only while a socket
//! is open on both ends, so it is held in memory rather than in the task
//! ledger. What the browser needs is (a) the current shells for a session, so
//! reloading the page reattaches to what is already running, and (b) the output
//! as it arrives, so the terminal is not blank until the next keystroke.
//!
//! Output is kept in a bounded per-shell scrollback. It is raw bytes on the way
//! in and on the way out — the browser renders it, so nothing here re-encodes
//! or re-wraps a terminal stream.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use uuid::Uuid;

/// Bytes of output retained per shell for reconnection.
///
/// Enough to fill a tall terminal several times over; beyond that the oldest
/// bytes are dropped rather than growing without bound for an idle shell.
const SCROLLBACK_BYTES: usize = 256 * 1024;

/// Ceiling on shells tracked per session, so a long session cannot accumulate.
const MAX_SHELLS_PER_SESSION: usize = 16;

/// One message to a browser watching a session.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShellMessage {
    /// The session's shells, sent on (re)attach.
    Snapshot {
        shells: std::sync::Arc<Vec<ShellView>>,
    },
    /// The implant confirmed a shell is running.
    Opened { shell: ShellView },
    /// Raw output, base64 of the bytes the shell wrote.
    Output {
        shell_id: Uuid,
        data: String,
        stderr: bool,
    },
    /// The shell ended, on request or on its own.
    Closed { shell_id: Uuid, code: Option<i32> },
    /// The implant could not do what was asked.
    Failed { shell_id: Uuid, reason: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct ShellView {
    pub id: Uuid,
    pub program: String,
    pub args: Vec<String>,
    /// Whether the child got a real terminal.
    pub pty: bool,
    /// `starting` until the implant confirms, then `running`, then `closed`.
    pub state: &'static str,
    pub opened_at: String,
    pub exit_code: Option<i32>,
    /// Everything this shell has written, base64, so an operator who opens the
    /// page later sees the session rather than a blank screen.
    pub scrollback: String,
}

/// Events the C2 socket handler forwards here, per session.
#[derive(Debug, Clone)]
pub enum ImplantEvent {
    ShellReady {
        shell_id: Uuid,
        program: String,
        pty: bool,
    },
    ShellData {
        shell_id: Uuid,
        data: String,
        stderr: bool,
    },
    ShellExit {
        shell_id: Uuid,
        code: Option<i32>,
    },
}

/// A kill confirmation is not shell state, so it is dropped here: the task
/// ledger is the record of what was killed, and the implant's own acknowledgement
/// only ever confirms what the server already recorded.
impl From<nw_profile::msgs::LiveEvent> for ImplantEvent {
    fn from(event: nw_profile::msgs::LiveEvent) -> Self {
        match event {
            nw_profile::msgs::LiveEvent::ShellReady {
                shell_id,
                program,
                pty,
            } => Self::ShellReady {
                shell_id,
                program,
                pty,
            },
            nw_profile::msgs::LiveEvent::ShellData {
                shell_id,
                data,
                stderr,
            } => Self::ShellData {
                shell_id,
                data,
                stderr,
            },
            nw_profile::msgs::LiveEvent::ShellExit { shell_id, code } => {
                Self::ShellExit { shell_id, code }
            }
            other => {
                tracing::debug!(?other, "live event is not shell state");
                Self::ShellExit {
                    shell_id: Uuid::nil(),
                    code: None,
                }
            }
        }
    }
}

struct ShellEntry {
    view: ShellView,
    /// Recent raw output, so a browser attaching late still sees context.
    scrollback: VecDeque<u8>,
}

#[derive(Default)]
struct SessionShells {
    shells: HashMap<Uuid, ShellEntry>,
    subscribers: HashMap<u64, tokio::sync::mpsc::UnboundedSender<ShellMessage>>,
}

impl SessionShells {
    /// Fan a message out, dropping watchers whose browser has gone away.
    fn publish(&mut self, message: ShellMessage) {
        self.subscribers
            .retain(|_, sender| sender.send(message.clone()).is_ok());
    }
}

#[derive(Clone, Default)]
pub struct ShellHub {
    sessions: Arc<Mutex<HashMap<String, SessionShells>>>,
}

/// One browser watching a session's shells.
pub struct Subscription {
    session_id: String,
    id: u64,
    messages: tokio::sync::mpsc::UnboundedReceiver<ShellMessage>,
    hub: ShellHub,
}

impl Subscription {
    pub async fn recv(&mut self) -> Option<ShellMessage> {
        self.messages.recv().await
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut sessions = self.hub.sessions.lock().expect("shell hub poisoned");
        if let Some(session) = sessions.get_mut(&self.session_id) {
            session.subscribers.remove(&self.id);
        }
        if let Some(session) = sessions.get(&self.session_id)
            && session.subscribers.is_empty()
            && session.shells.is_empty()
        {
            sessions.remove(&self.session_id);
        }
    }
}

impl ShellHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Watch a session's shells, replaying what is already known.
    pub fn subscribe(&self, session_id: &str) -> Subscription {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut sessions = self.sessions.lock().expect("shell hub poisoned");
        let entry = sessions.entry(session_id.to_owned()).or_default();
        entry.subscribers.insert(id, tx.clone());
        let snapshot = ShellMessage::Snapshot {
            shells: Self::with_scrollback(entry),
        };
        let _ = tx.send(snapshot);
        drop(sessions);
        Subscription {
            session_id: session_id.to_owned(),
            id,
            messages: rx,
            hub: self.clone(),
        }
    }

    /// Record a shell the portal is about to ask the implant for.
    ///
    /// The shell is listed as `starting` so the operator sees the request
    /// immediately, before the implant's confirmation round trip.
    pub fn expect_shell(&self, session_id: &str, id: Uuid, program: &str, args: &[String]) {
        let mut sessions = self.sessions.lock().expect("shell hub poisoned");
        let entry = sessions.entry(session_id.to_owned()).or_default();
        if entry.shells.len() >= MAX_SHELLS_PER_SESSION {
            // Prefer forgetting a finished shell over refusing to attach.
            let oldest = entry
                .shells
                .iter()
                .filter(|(_, shell)| shell.view.state == "closed")
                .min_by_key(|(_, shell)| shell.view.opened_at.clone())
                .map(|(id, _)| *id);
            if let Some(oldest) = oldest {
                entry.shells.remove(&oldest);
            }
        }
        entry.shells.insert(
            id,
            ShellEntry {
                view: ShellView {
                    id,
                    program: program.to_owned(),
                    args: args.to_vec(),
                    pty: false,
                    state: "starting",
                    opened_at: chrono::Utc::now().to_rfc3339(),
                    exit_code: None,
                    scrollback: String::new(),
                },
                scrollback: VecDeque::new(),
            },
        );
    }

    /// The shell an operator is typing into: the most recent one still open.
    pub fn active_shell(&self, session_id: &str) -> Option<Uuid> {
        let sessions = self.sessions.lock().expect("shell hub poisoned");
        let entry = sessions.get(session_id)?;
        entry
            .shells
            .iter()
            .filter(|(_, shell)| shell.view.state != "closed")
            .max_by_key(|(_, shell)| shell.view.opened_at.clone())
            .map(|(id, _)| *id)
    }

    pub fn shells(&self, session_id: &str) -> Vec<ShellView> {
        let sessions = self.sessions.lock().expect("shell hub poisoned");
        let Some(entry) = sessions.get(session_id) else {
            return Vec::new();
        };
        let mut shells: Vec<ShellView> = entry
            .shells
            .values()
            .map(|shell| ShellView {
                scrollback: encode(&shell.scrollback),
                ..shell.view.clone()
            })
            .collect();
        shells.sort_by(|left, right| left.opened_at.cmp(&right.opened_at));
        shells
    }

    /// Everything one shell has written, for a browser attaching late.
    pub fn scrollback(&self, session_id: &str, shell_id: Uuid) -> String {
        let sessions = self.sessions.lock().expect("shell hub poisoned");
        sessions
            .get(session_id)
            .and_then(|entry| entry.shells.get(&shell_id))
            .map(|shell| encode(&shell.scrollback))
            .unwrap_or_default()
    }

    /// Fill in the replayable output on every view, for a snapshot.
    fn with_scrollback(entry: &SessionShells) -> std::sync::Arc<Vec<ShellView>> {
        std::sync::Arc::new(
            entry
                .shells
                .values()
                .map(|shell| ShellView {
                    scrollback: encode(&shell.scrollback),
                    ..shell.view.clone()
                })
                .collect(),
        )
    }

    /// Record one event the implant pushed, and fan it out to watchers.
    pub fn record(&self, session_id: &str, event: &ImplantEvent) {
        let mut sessions = self.sessions.lock().expect("shell hub poisoned");
        let entry = sessions.entry(session_id.to_owned()).or_default();
        let now = chrono::Utc::now().to_rfc3339();
        match event {
            ImplantEvent::ShellReady {
                shell_id,
                program,
                pty,
            } => {
                let shell = entry.shells.entry(*shell_id).or_insert_with(|| ShellEntry {
                    view: ShellView {
                        id: *shell_id,
                        program: program.clone(),
                        args: Vec::new(),
                        pty: *pty,
                        state: "running",
                        opened_at: now.clone(),
                        exit_code: None,
                        scrollback: String::new(),
                    },
                    scrollback: VecDeque::new(),
                });
                shell.view.pty = *pty;
                shell.view.state = "running";
                shell.view.program = program.clone();
                let view = shell.view.clone();
                entry.publish(ShellMessage::Opened { shell: view });
            }
            ImplantEvent::ShellData {
                shell_id,
                data,
                stderr,
            } => {
                let Ok(bytes) = decode(data) else {
                    return;
                };
                let shell = entry.shells.entry(*shell_id).or_insert_with(|| ShellEntry {
                    view: ShellView {
                        id: *shell_id,
                        program: String::new(),
                        args: Vec::new(),
                        pty: false,
                        state: "running",
                        opened_at: now.clone(),
                        exit_code: None,
                        scrollback: String::new(),
                    },
                    scrollback: VecDeque::new(),
                });
                shell.scrollback.extend(bytes);
                while shell.scrollback.len() > SCROLLBACK_BYTES {
                    shell.scrollback.pop_front();
                }
                entry.publish(ShellMessage::Output {
                    shell_id: *shell_id,
                    data: data.clone(),
                    stderr: *stderr,
                });
            }
            ImplantEvent::ShellExit { shell_id, code } => {
                let shell = entry.shells.entry(*shell_id).or_insert_with(|| ShellEntry {
                    view: ShellView {
                        id: *shell_id,
                        program: String::new(),
                        args: Vec::new(),
                        pty: false,
                        state: "closed",
                        opened_at: now.clone(),
                        exit_code: *code,
                        scrollback: String::new(),
                    },
                    scrollback: VecDeque::new(),
                });
                shell.view.state = "closed";
                shell.view.exit_code = *code;
                shell.view.scrollback = encode(&shell.scrollback);
                entry.publish(ShellMessage::Closed {
                    shell_id: *shell_id,
                    code: *code,
                });
            }
        }
    }

    /// Report a request the portal could not fulfil.
    pub fn fail(&self, session_id: &str, shell_id: Uuid, reason: impl Into<String>) {
        let reason = reason.into();
        let mut sessions = self.sessions.lock().expect("shell hub poisoned");
        let entry = sessions.entry(session_id.to_owned()).or_default();
        entry.publish(ShellMessage::Failed { shell_id, reason });
    }

    /// Forget a session's shells, e.g. when the callback is deleted.
    pub fn forget(&self, session_id: &str) {
        self.sessions
            .lock()
            .expect("shell hub poisoned")
            .remove(session_id);
    }
}

fn encode(scrollback: &VecDeque<u8>) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .encode(scrollback.iter().copied().collect::<Vec<u8>>())
}

fn decode(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[tokio::test]
    async fn a_browser_attaching_late_still_sees_the_running_shell() {
        let hub = ShellHub::new();
        let session = "session-1";
        let shell = Uuid::new_v4();
        hub.expect_shell(session, shell, "/bin/sh", &[]);
        hub.record(
            session,
            &ImplantEvent::ShellReady {
                shell_id: shell,
                program: "/bin/sh".into(),
                pty: true,
            },
        );
        hub.record(
            session,
            &ImplantEvent::ShellData {
                shell_id: shell,
                data: "cm9vdEBob3N0".into(),
                stderr: false,
            },
        );

        let mut subscription = hub.subscribe(session);
        let ShellMessage::Snapshot { shells } = subscription.recv().await.unwrap() else {
            panic!("the first message is always the snapshot");
        };
        assert_eq!(shells.len(), 1);
        assert_eq!(shells[0].state, "running");
        assert!(shells[0].pty);
        assert_eq!(hub.scrollback(session, shell), "cm9vdEBob3N0");
    }

    #[tokio::test]
    async fn watchers_see_output_and_exit_as_they_happen() {
        let hub = ShellHub::new();
        let session = "session-2";
        let shell = Uuid::new_v4();
        let mut subscription = hub.subscribe(session);
        // The snapshot for an empty session still arrives first.
        assert!(matches!(
            subscription.recv().await,
            Some(ShellMessage::Snapshot { .. })
        ));

        hub.record(
            session,
            &ImplantEvent::ShellData {
                shell_id: shell,
                data: "aGk=".into(),
                stderr: false,
            },
        );
        hub.record(
            session,
            &ImplantEvent::ShellExit {
                shell_id: shell,
                code: Some(0),
            },
        );

        assert!(matches!(
            subscription.recv().await,
            Some(ShellMessage::Output { stderr: false, .. })
        ));
        assert!(matches!(
            subscription.recv().await,
            Some(ShellMessage::Closed { code: Some(0), .. })
        ));
        assert_eq!(
            hub.active_shell(session),
            None,
            "a closed shell is not active"
        );
    }

    #[tokio::test]
    async fn scrollback_stays_bounded() {
        let hub = ShellHub::new();
        let session = "session-3";
        let shell = Uuid::new_v4();
        let chunk = base64::engine::general_purpose::STANDARD.encode(vec![b'x'; 64 * 1024]);
        for _ in 0..8 {
            hub.record(
                session,
                &ImplantEvent::ShellData {
                    shell_id: shell,
                    data: chunk.clone(),
                    stderr: false,
                },
            );
        }
        // Bounded at 256 KiB of raw bytes, base64 of which is ~341 KiB.
        let stored = hub.scrollback(session, shell);
        assert!(
            stored.len() <= 350 * 1024,
            "{} bytes retained",
            stored.len()
        );
        assert_eq!(hub.shells(session).len(), 1);
    }

    #[tokio::test]
    async fn a_shell_is_only_pending_until_the_implant_confirms() {
        let hub = ShellHub::new();
        let session = "session-4";
        let shell = Uuid::new_v4();
        hub.expect_shell(session, shell, "/bin/sh", &["-i".to_owned()]);
        assert_eq!(hub.active_shell(session), Some(shell));
        assert_eq!(hub.shells(session)[0].state, "starting");

        hub.record(
            session,
            &ImplantEvent::ShellReady {
                shell_id: shell,
                program: "/bin/sh".into(),
                pty: true,
            },
        );
        assert_eq!(hub.shells(session)[0].state, "running");
        assert_eq!(hub.active_shell(session), Some(shell));
    }
}
