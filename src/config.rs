use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub bind: SocketAddr,
    pub evidence_dir: PathBuf,
    pub session_secret: String,
    pub cookie_secure: bool,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Config {
            database_url: env::var("NAUGHTYWOLF_DATABASE_URL")
                .unwrap_or_else(|_| "sqlite:naughtywolf.db?mode=rwc".to_string()),
            bind: env::var("NAUGHTYWOLF_BIND")
                .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
                .parse()
                .map_err(ConfigError::InvalidBind)?,
            evidence_dir: env::var("NAUGHTYWOLF_EVIDENCE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("evidence")),
            session_secret: env::var("NAUGHTYWOLF_SESSION_SECRET")
                .map_err(|_| ConfigError::Missing("NAUGHTYWOLF_SESSION_SECRET"))?,
            cookie_secure: match env::var("NAUGHTYWOLF_COOKIE_SECURE") {
                Ok(value) => parse_bool(&value)?,
                Err(env::VarError::NotPresent) => false,
                Err(env::VarError::NotUnicode(_)) => {
                    return Err(ConfigError::InvalidBoolean("NAUGHTYWOLF_COOKIE_SECURE"));
                }
            },
        })
    }

    /// Deterministic local settings for tests and in-process fixtures.
    pub fn for_test() -> Self {
        Self {
            database_url: "sqlite::memory:".to_string(),
            bind: "127.0.0.1:8080".parse().expect("valid loopback address"),
            evidence_dir: PathBuf::from("evidence"),
            session_secret: "test-session-secret-not-for-production".to_string(),
            cookie_secure: false,
        }
    }

    pub fn session_secret_bytes(&self) -> Vec<u8> {
        self.session_secret.as_bytes().to_vec()
    }
}

fn parse_bool(value: &str) -> Result<bool, ConfigError> {
    match value {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(ConfigError::InvalidBoolean("NAUGHTYWOLF_COOKIE_SECURE")),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Missing required environment variable: {0}")]
    Missing(&'static str),
    #[error("NAUGHTYWOLF_BIND must be a socket address: {0}")]
    InvalidBind(#[source] std::net::AddrParseError),
    #[error("{0} must be true, false, 1, or 0")]
    InvalidBoolean(&'static str),
}
