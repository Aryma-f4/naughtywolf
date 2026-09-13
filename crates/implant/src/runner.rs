use tokio::process::Command;
use tokio::task::AbortHandle;
use tokio::time::timeout;

use nw_profile::msgs::{Task, TaskResult};

/// Execute a remote task with a timeout, capturing stdout/stderr/exit code.
/// A hard timeout yields a synthetic failure result. The child is
/// `kill_on_drop`, so aborting the caller's task future kills the child (used
/// by `nw/killtask` cancellation).
pub async fn run(task: Task) -> TaskResult {
    let timeout_ms = if task.timeout_ms == 0 {
        30_000
    } else {
        task.timeout_ms
    };

    let (program, shell_args) = shell_invocation(&task.command, &task.args);
    let mut cmd = Command::new(program);
    cmd.args(&shell_args);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return TaskResult {
                task_id: task.id,
                ok: false,
                stdout: Vec::new(),
                stderr: format!("failed to spawn: {}", e).into_bytes(),
                exit_code: -1,
            };
        }
    };

    match timeout(
        std::time::Duration::from_millis(timeout_ms),
        child.wait_with_output(),
    )
    .await
    {
        Ok(Ok(out)) => TaskResult {
            task_id: task.id,
            ok: out.status.success(),
            stdout: out.stdout,
            stderr: out.stderr,
            exit_code: out.status.code().unwrap_or(-1),
        },
        Ok(Err(e)) => TaskResult {
            task_id: task.id,
            ok: false,
            stdout: Vec::new(),
            stderr: format!("failed to await: {}", e).into_bytes(),
            exit_code: -1,
        },
        Err(_) => TaskResult {
            task_id: task.id,
            ok: false,
            stdout: Vec::new(),
            stderr: format!("task timed out after {}ms", timeout_ms).into_bytes(),
            exit_code: -1,
        },
    }
}

/// Spawn a tracked task: the JoinHandle is registered under `task.id` so
/// `kill_task` can abort it, then detached. Returns the join handle so the
/// caller can await the outcome.
pub fn spawn_tracked(
    running: &std::sync::Mutex<std::collections::HashMap<uuid::Uuid, AbortHandle>>,
    task: Task,
) -> tokio::task::JoinHandle<TaskResult> {
    let task_id = task.id;
    let handle = tokio::spawn(async move { run(task).await });
    let abort = handle.abort_handle();
    running.lock().unwrap().insert(task_id, abort);
    handle
}

/// Quote a single command-line token for the platform shell.
#[cfg(not(windows))]
fn quote_for_shell(token: &str) -> String {
    if !token.contains([' ', '\'']) {
        return token.to_owned();
    }
    format!("'{}'", token.replace('\'', "'\\''"))
}

#[cfg(windows)]
fn quote_for_shell(token: &str) -> String {
    if !token.contains([' ', '"']) {
        return token.to_owned();
    }
    format!("\"{}\"", token.replace('"', "\"\""))
}

/// Rebuild a generic task as a platform shell invocation so operator-typed
/// commands run with native shell syntax (pipelines, redirection, `dir`,
/// `ipconfig`, …) instead of requiring an absolute binary path. Windows
/// callbacks get `cmd.exe /C`, POSIX hosts get `/bin/sh -c`.
///
/// Tasks that already *are* a shell (`sh -c …`, `cmd.exe /C …`, …) run
/// directly: re-wrapping stacks shells and, worse, relocates the real child
/// under the wrapper, so `kill_on_drop`/timeout would kill the wrapper and
/// orphan the actual command.
///
/// ponytail: `kill_on_drop` still targets only the immediate child; wrapping
/// a pipeline puts a shell in that slot, so grandchildren outlive a timeout.
/// Use `nw/process-kill` for process trees.
pub(super) fn shell_invocation(command: &str, args: &[String]) -> (String, Vec<String>) {
    if is_shell_program(command) {
        return (command.to_owned(), args.to_vec());
    }
    let mut line = String::new();
    line.push_str(&quote_for_shell(command));
    for arg in args {
        line.push(' ');
        line.push_str(&quote_for_shell(arg));
    }
    #[cfg(windows)]
    let program = "cmd.exe";
    #[cfg(not(windows))]
    let program = "/bin/sh";
    #[cfg(windows)]
    let program_args = vec!["/C".to_owned(), line];
    #[cfg(not(windows))]
    let program_args = vec!["-c".to_owned(), line];
    (program.to_owned(), program_args)
}

/// The command itself is a shell, run it verbatim instead of re-wrapping.
fn is_shell_program(command: &str) -> bool {
    matches!(
        command,
        "sh" | "/bin/sh"
            | "bash"
            | "/bin/bash"
            | "dash"
            | "/bin/dash"
            | "zsh"
            | "/bin/zsh"
            | "cmd"
            | "cmd.exe"
            | "powershell"
            | "powershell.exe"
            | "pwsh"
    )
}

/// Pure-std implementation used by lightweight tests (no tokio runtime).
pub fn run_blocking(task: &Task) -> TaskResult {
    let (program, shell_args) = shell_invocation(&task.command, &task.args);
    let output = std::process::Command::new(program)
        .args(&shell_args)
        .stdin(std::process::Stdio::null())
        .output();

    match output {
        Ok(out) => TaskResult {
            task_id: task.id,
            ok: out.status.success(),
            stdout: out.stdout,
            stderr: out.stderr,
            exit_code: out.status.code().unwrap_or(-1),
        },
        Err(e) => TaskResult {
            task_id: task.id,
            ok: false,
            stdout: Vec::new(),
            stderr: format!("failed to spawn: {}", e).into_bytes(),
            exit_code: -1,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn echo_captures_stdout() {
        let t = Task {
            id: Uuid::new_v4(),
            command: "printf".into(),
            args: vec!["hello-c2".into()],
            timeout_ms: 1000,
        };
        let r = run_blocking(&t);
        assert!(r.ok, "stderr: {}", String::from_utf8_lossy(&r.stderr));
        assert_eq!(String::from_utf8_lossy(&r.stdout), "hello-c2");
    }

    #[test]
    fn generic_commands_route_through_the_platform_shell() {
        #[cfg(windows)]
        let (program, args) = shell_invocation("ipconfig", &[]);
        #[cfg(not(windows))]
        let (program, args) =
            shell_invocation("ls", &["-la".to_owned(), "dir with'space".to_owned()]);
        #[cfg(windows)]
        {
            assert_eq!(program, "cmd.exe");
            assert_eq!(args, vec!["/C".to_owned(), "ipconfig".to_owned()]);
            let (p, a) = shell_invocation("C:\\Program Files\\app.exe", &["-v".into()]);
            assert_eq!(p, "cmd.exe");
            assert_eq!(a[1], "\"C:\\Program Files\\app.exe\" -v");
        }
        #[cfg(not(windows))]
        {
            assert_eq!(program, "/bin/sh");
            assert_eq!(
                args,
                vec!["-c".to_owned(), "ls -la 'dir with'\\''space'".to_owned(),]
            );
        }
        // A whitespace-free command line stays unquoted.
        assert_eq!(shell_invocation("whoami", &[]).1[1], "whoami");
    }

    #[test]
    fn explicit_shell_tasks_skip_the_wrapper() {
        let (program, args) = shell_invocation("sh", &["-c".to_owned(), "echo hi".to_owned()]);
        assert_eq!(program, "sh");
        assert_eq!(args, vec!["-c".to_owned(), "echo hi".to_owned()]);
        let (program, args) = shell_invocation("cmd.exe", &["/C".to_owned(), "dir".to_owned()]);
        assert_eq!(program, "cmd.exe");
        assert_eq!(args, vec!["/C".to_owned(), "dir".to_owned()]);
        assert!(
            is_shell_program("powershell"),
            "operator-chosen shells must not be double-wrapped"
        );
    }

    #[test]
    fn missing_binary_errors() {
        let t = Task {
            id: Uuid::new_v4(),
            command: "nw-definitely-missing-binary".into(),
            args: vec![],
            timeout_ms: 1000,
        };
        let r = run_blocking(&t);
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn command_is_terminated_at_its_task_timeout() {
        let task = Task {
            id: Uuid::new_v4(),
            command: "sleep".into(),
            args: vec!["1".into()],
            timeout_ms: 10,
        };
        let result = run(task).await;
        assert!(!result.ok);
        assert!(String::from_utf8_lossy(&result.stderr).contains("timed out"));
    }
}
