use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use crate::db::repositories::Repository;

const MAX_FRAME: usize = 8 * 1024 * 1024;

pub async fn serve_tcp(
    repository: Repository,
    psk: Arc<Vec<u8>>,
    bind: String,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    serve_tcp_listener(repository, psk, listener, "tcp".into()).await
}

pub async fn serve_tcp_listener(
    repository: Repository,
    psk: Arc<Vec<u8>>,
    listener: tokio::net::TcpListener,
    protocol: String,
) -> anyhow::Result<()> {
    tracing::info!(bind = %listener.local_addr()?, "portal raw-tcp C2 listener up");
    loop {
        let (mut socket, peer) = listener.accept().await?;
        let repository = repository.clone();
        let psk = psk.clone();
        let protocol = protocol.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_connection(&mut socket, &repository, &psk, &protocol).await {
                tracing::debug!(%peer, %error, "portal raw-tcp connection failed");
            }
        });
    }
}

async fn handle_connection(
    socket: &mut tokio::net::TcpStream,
    repository: &Repository,
    psk: &[u8],
    protocol: &str,
) -> anyhow::Result<()> {
    let wire = read_frame(socket).await?;
    let reply = crate::c2::process_sealed(repository, psk, &wire, protocol)
        .await
        .map_err(|error| anyhow::anyhow!("C2 dispatch: {error:?}"))?;
    socket.write_all(&frame_bytes(&reply)).await?;
    socket.flush().await?;
    Ok(())
}

async fn read_frame<R>(reader: &mut R) -> anyhow::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut length = [0u8; 4];
    reader.read_exact(&mut length).await?;
    let length = u32::from_be_bytes(length) as usize;
    anyhow::ensure!(length <= MAX_FRAME, "frame too large ({length})");
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    Ok(body)
}

fn frame_bytes(body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(body);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_frames_larger_than_eight_mebibytes() {
        let (mut writer, mut reader) = tokio::io::duplex(16);
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            writer
                .write_all(&((8 * 1024 * 1024 + 1) as u32).to_be_bytes())
                .await
                .unwrap();
        });
        let error = read_frame(&mut reader).await.unwrap_err();
        assert!(error.to_string().contains("frame too large"));
    }

    #[test]
    fn reply_frame_has_big_endian_length_prefix() {
        let frame = frame_bytes(b"sealed");
        assert_eq!(&frame[..4], &6u32.to_be_bytes());
        assert_eq!(&frame[4..], b"sealed");
    }
}
