use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct SliverCfg {
    pub operator: String,
    pub lhost: String,
    pub lport: u16,
    pub token: String,
    pub ca_certificate: String,
    pub certificate: String,
    pub private_key: String,
    #[serde(default)]
    pub wg: Option<WgConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WgConfig {
    pub server_pub_key: Option<String>,
    pub client_private_key: Option<String>,
    pub client_ip: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ParsedProfile {
    pub name: String,
    pub config_path: PathBuf,
    pub operator_name: String,
    pub lhost: String,
    pub lport: u16,
}

impl SliverCfg {
    pub fn from_file(path: &Path) -> Result<Self, ProfileError> {
        let content =
            fs::read_to_string(path).map_err(|e| ProfileError::Io(path.to_path_buf(), e))?;
        serde_json::from_str(&content).map_err(|e| ProfileError::Parse(path.to_path_buf(), e))
    }

    pub fn profile_name(&self) -> String {
        format!("{}_{}", self.operator, self.lhost)
    }
}

/// Scan a directory for Sliver operator .cfg files
pub fn scan_config_dir(dir: &Path) -> Result<Vec<ParsedProfile>, ProfileError> {
    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut profiles = Vec::new();

    for entry in fs::read_dir(dir).map_err(|e| ProfileError::Io(dir.to_path_buf(), e))? {
        let entry = entry.map_err(|e| ProfileError::Io(dir.to_path_buf(), e))?;
        let path = entry.path();

        if path.extension().is_some_and(|ext| ext == "cfg") {
            match SliverCfg::from_file(&path) {
                Ok(cfg) => {
                    profiles.push(ParsedProfile {
                        name: cfg.profile_name(),
                        config_path: path,
                        operator_name: cfg.operator,
                        lhost: cfg.lhost,
                        lport: cfg.lport,
                    });
                }
                Err(e) => {
                    tracing::warn!("Skipping invalid config {}: {}", path.display(), e);
                }
            }
        }
    }

    Ok(profiles)
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("IO error reading {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("Parse error in {0}: {1}")]
    Parse(PathBuf, serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_valid_cfg() {
        let json = r#"{
            "operator": "alice",
            "lhost": "192.168.1.100",
            "lport": 31337,
            "token": "abc123",
            "ca_certificate": "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----",
            "certificate": "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----",
            "private_key": "-----BEGIN EC PRIVATE KEY-----\nMIIB\n-----END EC PRIVATE KEY-----"
        }"#;

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(json.as_bytes()).unwrap();
        file.flush().unwrap();

        let cfg = SliverCfg::from_file(file.path()).unwrap();
        assert_eq!(cfg.operator, "alice");
        assert_eq!(cfg.lhost, "192.168.1.100");
        assert_eq!(cfg.lport, 31337);
        assert_eq!(cfg.token, "abc123");
        assert_eq!(cfg.profile_name(), "alice_192.168.1.100");
    }

    #[test]
    fn test_parse_invalid_cfg() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"not json").unwrap();
        file.flush().unwrap();
        assert!(SliverCfg::from_file(file.path()).is_err());
    }

    #[test]
    fn test_scan_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let profiles = scan_config_dir(dir.path()).unwrap();
        assert!(profiles.is_empty());
    }

    #[test]
    fn test_scan_with_configs() {
        let dir = tempfile::tempdir().unwrap();
        let json = r#"{"operator":"bob","lhost":"10.0.0.1","lport":4444,"token":"x","ca_certificate":"c","certificate":"c","private_key":"k"}"#;
        let path = dir.path().join("bob_10.0.0.1.cfg");
        std::fs::write(&path, json).unwrap();

        let profiles = scan_config_dir(dir.path()).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].operator_name, "bob");
    }
}
