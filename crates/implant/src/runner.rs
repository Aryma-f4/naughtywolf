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

    let mut cmd = Command::new(&task.command);
    cmd.args(&task.args);
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
