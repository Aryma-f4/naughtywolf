pub struct ServerConfig {
    pub bind: String,
    pub tcp_bind: Option<String>,
    pub dns_bind: Option<String>,
    pub callback_host: String,
    pub psk: String,
    pub downloads_dir: String,
    /// Path to SQLite database for session/task/operator persistence.
    /// When None, falls back to in-memory. Set to ":memory:" for in-process tests.
    pub db_path: Option<String>,
    /// Initial admin password used only when the operator database is empty.
    pub admin_password: Option<String>,
}

impl ServerConfig {
    pub fn from_env() -> Self {
        Self::from_env_with(|key| std::env::var(key).ok())
    }

    fn from_env_with<F>(mut get: F) -> Self
    where
        F: FnMut(&str) -> Option<String>,
    {
        ServerConfig {
            bind: get("NW_BIND").unwrap_or_else(|| "127.0.0.1:8081".into()),
            tcp_bind: get("NW_TCP_BIND").filter(|s| !s.is_empty()),
            dns_bind: get("NW_DNS_BIND").filter(|s| !s.is_empty()),
            callback_host: get("NW_CALLBACK_HOST")
                .unwrap_or_else(|| "http://127.0.0.1:8081".into()),
            psk: get("NW_PSK").unwrap_or_else(|| "dev-psk-change-me".into()),
            downloads_dir: get("NW_DOWNLOADS").unwrap_or_else(|| "downloads".into()),
            db_path: get("NW_DB").filter(|s| !s.is_empty()),
            admin_password: get("NW_ADMIN_PASSWORD").filter(|s| !s.is_empty()),
        }
    }

    /// Returns the SQLite path to use, or None for pure in-memory.
    pub fn db_path(&self) -> Option<&str> {
        self.db_path.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::ServerConfig;

    #[test]
    fn uses_local_defaults_when_environment_is_unset() {
        let config = ServerConfig::from_env_with(|_| None);

        assert_eq!(config.bind, "127.0.0.1:8081");
        assert_eq!(config.callback_host, "http://127.0.0.1:8081");
        assert_eq!(config.psk, "dev-psk-change-me");
        assert_eq!(config.db_path, None);
        assert_eq!(config.admin_password, None);
    }

    #[test]
    fn uses_environment_values_when_present() {
        let config = ServerConfig::from_env_with(|key| match key {
            "NW_BIND" => Some("0.0.0.0:9081".into()),
            "NW_CALLBACK_HOST" => Some("https://c2.example.test".into()),
            "NW_PSK" => Some("test-psk".into()),
            "NW_DB" => Some(":memory:".into()),
            "NW_ADMIN_PASSWORD" => Some("correct-horse-battery-staple".into()),
            _ => None,
        });

        assert_eq!(config.bind, "0.0.0.0:9081");
        assert_eq!(config.callback_host, "https://c2.example.test");
        assert_eq!(config.psk, "test-psk");
        assert_eq!(config.db_path, Some(":memory:".to_string()));
        assert_eq!(
            config.admin_password,
            Some("correct-horse-battery-staple".to_string())
        );
    }
}
