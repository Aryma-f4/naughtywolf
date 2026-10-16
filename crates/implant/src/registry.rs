//! Windows registry tasking.
//!
//! Operator surface is `nw/registry <action> <path> [args]`:
//!
//! ```text
//! nw/registry query  HKCU\Software\Microsoft\Windows\CurrentVersion\Run
//! nw/registry query  HKLM\SYSTEM\CurrentControlSet\Control\ComputerName\ComputerName Hostname
//! nw/registry set    HKCU\Software\NaughtyWolf Beacon "C:\x.exe --quiet"
//! nw/registry delete HKCU\Software\NaughtyWolf Beacon
//! nw/registry delete HKCU\Software\NaughtyWolf
//! ```
//!
//! Parsing is deliberately separate from the Windows API calls so the accepted
//! command grammar is testable on any host, and so a malformed request from an
//! operator never reaches `advapi32`.

#[cfg(target_os = "windows")]
use std::fmt::Write as _;

/// A registry hive, named the way an operator writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hive {
    ClassesRoot,
    CurrentUser,
    LocalMachine,
    Users,
    CurrentConfig,
}

impl Hive {
    /// Canonical short name, as written in commands.
    pub fn short_name(self) -> &'static str {
        match self {
            Hive::ClassesRoot => "HKCR",
            Hive::CurrentUser => "HKCU",
            Hive::LocalMachine => "HKLM",
            Hive::Users => "HKU",
            Hive::CurrentConfig => "HKCC",
        }
    }
}

/// Parse a hive name, accepting the usual aliases.
pub fn parse_hive(name: &str) -> Result<Hive, String> {
    match name.trim().to_ascii_uppercase().as_str() {
        "HKCR" | "HKEY_CLASSES_ROOT" | "CLASSES_ROOT" => Ok(Hive::ClassesRoot),
        "HKCU" | "HKEY_CURRENT_USER" | "CURRENT_USER" => Ok(Hive::CurrentUser),
        "HKLM" | "HKEY_LOCAL_MACHINE" | "LOCAL_MACHINE" => Ok(Hive::LocalMachine),
        "HKU" | "HKEY_USERS" | "USERS" => Ok(Hive::Users),
        "HKCC" | "HKEY_CURRENT_CONFIG" | "CURRENT_CONFIG" => Ok(Hive::CurrentConfig),
        other => Err(format!(
            "unknown hive {other:?} (expected HKCR, HKCU, HKLM, HKU or HKCC)"
        )),
    }
}

/// Split `HKCU\Software\Foo` into its hive and subkey.
///
/// A path without a hive prefix is rejected rather than silently assumed to be
/// `HKCU`, because writing to the wrong hive is worse than an error.
pub fn split_path(path: &str) -> Result<(Hive, String), String> {
    let path = path.trim().trim_start_matches('\\');
    let (head, rest) = path
        .split_once('\\')
        .ok_or_else(|| format!("path {path:?} has no hive prefix"))?;
    let hive = parse_hive(head)?;
    let subkey = rest.trim_matches('\\').to_owned();
    if subkey.is_empty() {
        return Err(format!("path {path:?} has no subkey"));
    }
    Ok((hive, subkey))
}

/// A parsed registry operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Enumerate a key, or read one value when `value` is set.
    Query {
        hive: Hive,
        subkey: String,
        value: Option<String>,
    },
    /// Create or update a string value.
    Set {
        hive: Hive,
        subkey: String,
        name: String,
        data: String,
    },
    /// Delete one value, or an empty key when `value` is `None`.
    Delete {
        hive: Hive,
        subkey: String,
        value: Option<String>,
    },
}

/// Parse registry task arguments into an [`Action`].
///
/// Grammar: `query <path> [value]` | `set <path> <name> <data>` |
/// `delete <path> [value]`.
pub fn parse_action(args: &[String]) -> Result<Action, String> {
    let Some(verb) = args.first().map(|s| s.trim().to_ascii_lowercase()) else {
        return Err(
            "usage: query <path> [value] | set <path> <name> <data> | delete <path> [value]".into(),
        );
    };
    let rest = &args[1..];

    match verb.as_str() {
        "query" | "q" => match rest {
            [path] => {
                let (hive, subkey) = split_path(path)?;
                Ok(Action::Query {
                    hive,
                    subkey,
                    value: None,
                })
            }
            [path, value] => {
                let (hive, subkey) = split_path(path)?;
                Ok(Action::Query {
                    hive,
                    subkey,
                    value: Some(value.clone()),
                })
            }
            _ => Err("usage: query <path> [value]".into()),
        },
        "set" => match rest {
            [path, name, data] => {
                let (hive, subkey) = split_path(path)?;
                if name.trim().is_empty() {
                    return Err("set requires a value name".into());
                }
                Ok(Action::Set {
                    hive,
                    subkey,
                    name: name.clone(),
                    data: data.clone(),
                })
            }
            _ => Err("usage: set <path> <name> <data>".into()),
        },
        "delete" | "del" => match rest {
            [path] => {
                let (hive, subkey) = split_path(path)?;
                Ok(Action::Delete {
                    hive,
                    subkey,
                    value: None,
                })
            }
            [path, value] => {
                let (hive, subkey) = split_path(path)?;
                Ok(Action::Delete {
                    hive,
                    subkey,
                    value: Some(value.clone()),
                })
            }
            _ => Err("usage: delete <path> [value]".into()),
        },
        other => Err(format!(
            "unknown registry action {other:?} (expected query, set or delete)"
        )),
    }
}

/// Run a parsed action, returning operator-facing output.
pub fn execute(action: &Action) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        windows::execute(action)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = action;
        Err(format!(
            "registry tasking is Windows-only (running on {})",
            std::env::consts::OS
        ))
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::*;

    fn hive_handle(hive: Hive) -> windows_sys::Win32::System::Registry::HKEY {
        match hive {
            Hive::ClassesRoot => HKEY_CLASSES_ROOT,
            Hive::CurrentUser => HKEY_CURRENT_USER,
            Hive::LocalMachine => HKEY_LOCAL_MACHINE,
            Hive::Users => HKEY_USERS,
            Hive::CurrentConfig => HKEY_CURRENT_CONFIG,
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn from_wide(buf: &[u16]) -> String {
        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    fn last_error(code: windows_sys::Win32::Foundation::WIN32_ERROR) -> String {
        format!("registry error {code}")
    }

    fn open(hive: Hive, subkey: &str, access: REG_SAM_FLAGS) -> Result<HKEY, String> {
        let subkey = wide(subkey);
        let mut handle: HKEY = std::ptr::null_mut();
        // SAFETY: both strings are NUL-terminated; `handle` is a valid out slot.
        let status = unsafe {
            RegOpenKeyExW(
                hive_handle(hive),
                subkey.as_ptr(),
                0,
                access,
                &mut handle as *mut _,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "open {}\\{subkey:?}: {}",
                hive.short_name(),
                last_error(status)
            ));
        }
        Ok(handle)
    }

    struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: `self.0` was opened by `open` and is closed once.
            unsafe { RegCloseKey(self.0) };
        }
    }

    fn type_name(kind: REG_VALUE_TYPE) -> &'static str {
        match kind {
            REG_SZ => "REG_SZ",
            REG_EXPAND_SZ => "REG_EXPAND_SZ",
            REG_DWORD => "REG_DWORD",
            REG_QWORD => "REG_QWORD",
            REG_BINARY => "REG_BINARY",
            REG_MULTI_SZ => "REG_MULTI_SZ",
            _ => "REG_OTHER",
        }
    }

    /// Render one value as `name<TAB>TYPE<TAB>data`.
    fn read_value(key: HKEY, name: Option<&str>) -> Result<String, String> {
        let name_wide = name.map(wide);
        let name_ptr = name_wide
            .as_ref()
            .map(|n| n.as_ptr())
            .unwrap_or(std::ptr::null());

        let mut kind: REG_VALUE_TYPE = 0;
        let mut size: u32 = 0;
        // SAFETY: sized query with null buffer to learn the length first.
        let status = unsafe {
            RegQueryValueExW(
                key,
                name_ptr,
                std::ptr::null(),
                &mut kind as *mut _,
                std::ptr::null_mut(),
                &mut size as *mut _,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(last_error(status));
        }

        let mut data = vec![0u8; size as usize];
        // SAFETY: `data` is `size` bytes, which is what the value reports.
        let status = unsafe {
            RegQueryValueExW(
                key,
                name_ptr,
                std::ptr::null(),
                &mut kind as *mut _,
                data.as_mut_ptr().cast(),
                &mut size as *mut _,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(last_error(status));
        }
        data.truncate(size as usize);

        let rendered = match kind {
            REG_SZ | REG_EXPAND_SZ => {
                let units: Vec<u16> = data
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                from_wide(&units)
            }
            REG_DWORD if data.len() >= 4 => {
                format!(
                    "0x{:08x}",
                    u32::from_le_bytes([data[0], data[1], data[2], data[3]])
                )
            }
            REG_QWORD if data.len() >= 8 => {
                let bytes: [u8; 8] = data[..8].try_into().expect("checked length");
                format!("0x{:016x}", u64::from_le_bytes(bytes))
            }
            _ => {
                let hex: String = data.iter().map(|b| format!("{b:02x}")).collect();
                hex
            }
        };

        Ok(format!(
            "{}\t{}\t{}",
            name.unwrap_or("(Default)"),
            type_name(kind),
            rendered
        ))
    }

    fn list_key(key: HKEY) -> Result<String, String> {
        let mut out = String::new();

        // Values first, then subkeys, each enumerated until exhausted.
        let mut index = 0u32;
        loop {
            let mut name = [0u16; 512];
            let mut len = name.len() as u32;
            // SAFETY: `name` is sized for `len` on every iteration.
            let status = unsafe {
                RegEnumValueW(
                    key,
                    index,
                    name.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if status == ERROR_NO_MORE_ITEMS {
                break;
            }
            if status != ERROR_SUCCESS {
                return Err(last_error(status));
            }
            let label = from_wide(&name[..len as usize]);
            match read_value(key, if label.is_empty() { None } else { Some(&label) }) {
                Ok(line) => {
                    let _ = writeln!(out, "{line}");
                }
                Err(e) => {
                    let _ = writeln!(out, "{label}\t<unreadable: {e}>");
                }
            }
            index += 1;
        }

        let mut index = 0u32;
        loop {
            let mut name = [0u16; 512];
            let mut len = name.len() as u32;
            // SAFETY: `name` is sized for `len` on every iteration.
            let status = unsafe {
                RegEnumKeyExW(
                    key,
                    index,
                    name.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if status == ERROR_NO_MORE_ITEMS {
                break;
            }
            if status != ERROR_SUCCESS {
                return Err(last_error(status));
            }
            let _ = writeln!(out, "[{}]", from_wide(&name[..len as usize]));
            index += 1;
        }

        if out.is_empty() {
            out.push_str("(empty key)\n");
        }
        Ok(out)
    }

    pub(super) fn execute(action: &Action) -> Result<String, String> {
        match action {
            Action::Query {
                hive,
                subkey,
                value,
            } => {
                let key = Key(open(*hive, subkey, KEY_READ)?);
                match value {
                    Some(name) => Ok(read_value(key.0, Some(name))? + "\n"),
                    None => list_key(key.0),
                }
            }
            Action::Set {
                hive,
                subkey,
                name,
                data,
            } => {
                let key = Key(open(*hive, subkey, KEY_SET_VALUE)?);
                let name_wide = wide(name);
                let data_wide = wide(data);
                let bytes = (data_wide.len() * 2) as u32;
                // SAFETY: both strings are NUL-terminated and `bytes` is exact.
                let status = unsafe {
                    RegSetValueExW(
                        key.0,
                        name_wide.as_ptr(),
                        0,
                        REG_SZ,
                        data_wide.as_ptr().cast(),
                        bytes,
                    )
                };
                if status != ERROR_SUCCESS {
                    return Err(last_error(status));
                }
                Ok(format!(
                    "set {}\\{subkey}\\{name} = {data}\n",
                    hive.short_name()
                ))
            }
            Action::Delete {
                hive,
                subkey,
                value,
            } => match value {
                Some(name) => {
                    let key = Key(open(*hive, subkey, KEY_SET_VALUE)?);
                    let name_wide = wide(name);
                    // SAFETY: `name_wide` is NUL-terminated.
                    let status = unsafe { RegDeleteValueW(key.0, name_wide.as_ptr()) };
                    if status != ERROR_SUCCESS {
                        return Err(last_error(status));
                    }
                    Ok(format!("deleted {}\\{subkey}\\{name}\n", hive.short_name()))
                }
                None => {
                    // SAFETY: `subkey_wide` is NUL-terminated and relative to
                    // the hive root, which this does not open.
                    let subkey_wide = wide(subkey);
                    let status = unsafe { RegDeleteKeyW(hive_handle(*hive), subkey_wide.as_ptr()) };
                    if status != ERROR_SUCCESS {
                        return Err(last_error(status));
                    }
                    Ok(format!("deleted {}\\{subkey}\n", hive.short_name()))
                }
            },
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
    fn hives_accept_the_usual_aliases() {
        assert_eq!(parse_hive("HKCU").unwrap(), Hive::CurrentUser);
        assert_eq!(
            parse_hive("hkey_local_machine").unwrap(),
            Hive::LocalMachine
        );
        assert_eq!(parse_hive(" HKCR ").unwrap(), Hive::ClassesRoot);
        assert_eq!(parse_hive("hku").unwrap(), Hive::Users);
        assert_eq!(parse_hive("current_config").unwrap(), Hive::CurrentConfig);
        assert!(parse_hive("HKXX").is_err());
        assert!(parse_hive("").is_err());
    }

    #[test]
    fn paths_split_into_hive_and_subkey() {
        let (hive, subkey) = split_path(r"HKCU\Software\NaughtyWolf").unwrap();
        assert_eq!(hive, Hive::CurrentUser);
        assert_eq!(subkey, r"Software\NaughtyWolf");

        // Leading and trailing separators are tolerated.
        let (_, subkey) = split_path(r"\HKLM\SYSTEM\CurrentControlSet\").unwrap();
        assert_eq!(subkey, r"SYSTEM\CurrentControlSet");
    }

    #[test]
    fn paths_without_a_hive_are_rejected() {
        // Silently defaulting to HKCU would write to the wrong hive.
        assert!(split_path(r"Software\NaughtyWolf").is_err());
        assert!(split_path("HKCU").is_err());
        assert!(split_path("HKCU\\").is_err());
    }

    #[test]
    fn query_parses_with_and_without_a_value() {
        assert_eq!(
            parse_action(&args(&["query", r"HKCU\Software\Foo"])).unwrap(),
            Action::Query {
                hive: Hive::CurrentUser,
                subkey: r"Software\Foo".into(),
                value: None,
            }
        );
        assert_eq!(
            parse_action(&args(&["query", r"HKCU\Software\Foo", "Bar"])).unwrap(),
            Action::Query {
                hive: Hive::CurrentUser,
                subkey: r"Software\Foo".into(),
                value: Some("Bar".into()),
            }
        );
    }

    #[test]
    fn set_requires_a_name_and_data() {
        assert_eq!(
            parse_action(&args(&["set", r"HKCU\Software\Foo", "Bar", "baz"])).unwrap(),
            Action::Set {
                hive: Hive::CurrentUser,
                subkey: r"Software\Foo".into(),
                name: "Bar".into(),
                data: "baz".into(),
            }
        );
        assert!(parse_action(&args(&["set", r"HKCU\Software\Foo", "Bar"])).is_err());
        assert!(parse_action(&args(&["set", r"HKCU\Software\Foo", "  ", "baz"])).is_err());
    }

    #[test]
    fn delete_treats_a_trailing_value_as_the_target() {
        assert_eq!(
            parse_action(&args(&["delete", r"HKCU\Software\Foo", "Bar"])).unwrap(),
            Action::Delete {
                hive: Hive::CurrentUser,
                subkey: r"Software\Foo".into(),
                value: Some("Bar".into()),
            }
        );
        assert_eq!(
            parse_action(&args(&["del", r"HKCU\Software\Foo"])).unwrap(),
            Action::Delete {
                hive: Hive::CurrentUser,
                subkey: r"Software\Foo".into(),
                value: None,
            }
        );
    }

    #[test]
    fn malformed_commands_report_usage() {
        for bad in [
            vec![],
            args(&["frobnicate", r"HKCU\Software"]),
            args(&["query"]),
            args(&["query", "no-hive-here"]),
            args(&["delete", r"HKCU\Software", "a", "b"]),
        ] {
            assert!(
                parse_action(&bad).is_err(),
                "should have been rejected: {bad:?}"
            );
        }
    }

    #[test]
    fn verbs_are_case_insensitive() {
        assert_eq!(
            parse_action(&args(&["QUERY", r"HKCU\Software\Foo"])).unwrap(),
            parse_action(&args(&["query", r"HKCU\Software\Foo"])).unwrap()
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn execution_reports_windows_only_off_windows() {
        let action = parse_action(&args(&["query", r"HKCU\Software\Foo"])).unwrap();
        let err = execute(&action).unwrap_err();
        assert!(err.contains("Windows-only"), "unexpected error: {err}");
    }
}
