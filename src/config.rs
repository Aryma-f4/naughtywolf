use std::env;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub bind: String,
    pub session_secret: String,
    pub sliver_config_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Config {
            database_url: env::var("DATABASE_URL")
                .map_err(|_| ConfigError::Missing("DATABASE_URL"))?,
            bind: env::var("NAUGHTYWOLF_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string()),
            session_secret: env::var("NAUGHTYWOLF_SESSION_SECRET")
                .map_err(|_| ConfigError::Missing("NAUGHTYWOLF_SESSION_SECRET"))?,
            sliver_config_dir: env::var("SLIVER_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    dirs::home_dir()
                        .unwrap_or_default()
                        .join(".sliver-client/configs")
                }),
        })
    }

    pub fn session_secret_bytes(&self) -> Vec<u8> {
        self.session_secret.as_bytes().to_vec()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Missing required environment variable: {0}")]
    Missing(&'static str),
}
