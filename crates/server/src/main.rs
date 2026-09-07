use nw_server::{ServerConfig, server};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = ServerConfig::from_env();

    let rt = tokio::runtime::Runtime::new()?;
    let state = rt.block_on(server::build_state(&config))?;
    rt.block_on(server::serve(state, &config))?;
    Ok(())
}
