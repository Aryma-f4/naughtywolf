pub struct ServerConfig {
    pub bind: String,
    pub tcp_bind: Option<String>,
    pub dns_bind: Option<String>,
    pub callback_host: String,
    pub psk: String,
    pub downloads_dir: String,
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
        }
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
    }

    #[test]
    fn uses_environment_values_when_present() {
        let config = ServerConfig::from_env_with(|key| match key {
            "NW_BIND" => Some("0.0.0.0:9081".into()),
            "NW_CALLBACK_HOST" => Some("https://c2.example.test".into()),
            "NW_PSK" => Some("test-psk".into()),
            _ => None,
        });

        assert_eq!(config.bind, "0.0.0.0:9081");
        assert_eq!(config.callback_host, "https://c2.example.test");
        assert_eq!(config.psk, "test-psk");
    }
}
