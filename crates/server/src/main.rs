use nw_server::{ServerConfig, ServerState, server};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = ServerConfig::from_env();

    let bind = config.bind.clone();
    let tcp_bind = config.tcp_bind.clone();
    let dns_bind = config.dns_bind.clone();
    let rt = tokio::runtime::Runtime::new()?;
    let state = rt.block_on(server::build_state(&config))?;
    rt.block_on(run(state, bind, tcp_bind, dns_bind))?;
    Ok(())
}

/// Run whichever listeners are configured, until any one of them errors out,
/// then surface that error. Listeners that return early (bind failure) bubble
/// up via `select!`.
async fn run(
    state: ServerState,
    bind: String,
    tcp_bind: Option<String>,
    dns_bind: Option<String>,
) -> anyhow::Result<()> {
    let psk = state.psk.to_vec();
    let registry = state.registry.clone();
    let queue = state.queue.clone();
    let http = server::serve(state.clone(), &bind);
    let tcp = tcp_bind.map(|b| nw_server::serve_tcp(state.clone(), b));
    let dns = dns_bind.map(|b| nw_server::serve_dns(registry, queue, psk, b));

    tokio::select! {
        r = http => r,
        r = async { match tcp { Some(f) => f.await, None => std::future::pending().await } } => r,
        r = async { match dns { Some(f) => f.await, None => std::future::pending().await } } => r,
    }
}
