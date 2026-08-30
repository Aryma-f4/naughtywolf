use tokio::process::Command;
use tokio::time::timeout;

use nw_profile::msgs::{Task, TaskResult};

/// Execute a remote task with a timeout, capturing stdout/stderr/exit code.
/// A hard timeout yields a synthetic failure result.
pub async fn run(task: Task) -> TaskResult {
    // Implant-local exit is handled by the runtime loop, not here.
    let timeout_ms = if task.timeout_ms == 0 { 30_000 } else { task.timeout_ms };

    let mut cmd = Command::new(&task.command);
    cmd.args(&task.args);
    cmd.stdin(std::process::Stdio::null());

    let fut = async {
        match cmd.output().await {
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
    };

    match timeout(std::time::Duration::from_millis(timeout_ms), fut).await {
        Ok(result) => result,
        Err(_) => TaskResult {
            task_id: task.id,
            ok: false,
            stdout: Vec::new(),
            stderr: format!("task timed out after {}ms", timeout_ms).into_bytes(),
            exit_code: -1,
        },
    }
}

/// Pure-std implementation used by lightweight tests (no tokio runtime).
pub fn run_blocking(task: &Task) -> TaskResult {
    let output = std::process::Command::new(&task.command)
        .args(&task.args)
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
}
