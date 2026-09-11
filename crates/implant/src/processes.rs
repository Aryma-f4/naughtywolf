use chrono::{DateTime, SecondsFormat, Utc};
use nw_profile::control::{
    ControlError, PROCESS_KILL_SCHEMA_V1, PROCESS_LIST_SCHEMA_V1, ProcessEntry, ProcessKillV1,
    ProcessListV1,
};
use nw_profile::msgs::{Task, TaskResult};
use std::{
    thread,
    time::{Duration, Instant},
};
use sysinfo::{
    MINIMUM_CPU_UPDATE_INTERVAL, Pid, ProcessStatus, ProcessesToUpdate, Signal, System, Users,
};

const TERMINATION_WAIT: Duration = Duration::from_secs(2);

fn timestamp(seconds: u64) -> Option<String> {
    DateTime::<Utc>::from_timestamp(seconds.try_into().ok()?, 0)
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn list() -> ProcessListV1 {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL);
    system.refresh_processes(ProcessesToUpdate::All, true);
    let users = Users::new_with_refreshed_list();
    let mut processes = system
        .processes()
        .values()
        .map(|process| ProcessEntry {
            pid: process.pid().as_u32(),
            parent_pid: process.parent().map(Pid::as_u32),
            name: process.name().to_string_lossy().into_owned(),
            executable: process
                .exe()
                .map(|path| path.to_string_lossy().into_owned()),
            user: process
                .user_id()
                .and_then(|user_id| users.get_user_by_id(user_id))
                .map(|user| user.name().to_owned()),
            architecture: None,
            cpu_percent: process.cpu_usage(),
            memory_bytes: process.memory(),
            started_at: timestamp(process.start_time()),
        })
        .collect::<Vec<_>>();
    processes.sort_by_key(|process| process.pid);
    ProcessListV1 {
        schema: PROCESS_LIST_SCHEMA_V1.to_owned(),
        captured_at: now(),
        processes,
    }
}

pub fn kill(pid: u32) -> Result<ProcessKillV1, ControlError> {
    let sys_pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[sys_pid]), true);
    let process = system
        .process(sys_pid)
        .ok_or(ControlError::NoSuchProcess { pid })?;
    let name = process.name().to_string_lossy().into_owned();

    let terminated = match process.kill_with(Signal::Term) {
        Some(true) => true,
        Some(false) => return Err(ControlError::TerminateFailed { pid }),
        None if process.kill() => true,
        None => return Err(ControlError::TerminateFailed { pid }),
    };
    if !wait_for_exit(pid) {
        return Err(ControlError::TerminateFailed { pid });
    }

    Ok(ProcessKillV1 {
        schema: PROCESS_KILL_SCHEMA_V1.to_owned(),
        pid,
        name,
        terminated,
        terminated_at: now(),
    })
}

fn wait_for_exit(pid: u32) -> bool {
    let sys_pid = Pid::from_u32(pid);
    let deadline = Instant::now() + TERMINATION_WAIT;
    loop {
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::All, true);
        match system.process(sys_pid) {
            None => return true,
            Some(process) if matches!(process.status(), ProcessStatus::Zombie) =>
            {
                return true;
            }
            Some(_) if Instant::now() >= deadline => return false,
            Some(_) => thread::sleep(Duration::from_millis(20)),
        }
    }
}

fn failure(task: &Task, code: &str, message: impl Into<String>) -> TaskResult {
    let stderr = serde_json::to_vec(&serde_json::json!({
        "code": code,
        "message": message.into(),
    }))
    .unwrap_or_else(|_| b"{\"code\":\"serialization_failed\"}".to_vec());
    TaskResult {
        task_id: task.id,
        ok: false,
        stdout: Vec::new(),
        stderr,
        exit_code: -1,
    }
}

pub fn execute(task: &Task) -> Option<TaskResult> {
    let output = match task.command.as_str() {
        "nw/process-list" if task.args.is_empty() => {
            serde_json::to_vec(&list()).map_err(|error| ("serialization_failed", error.to_string()))
        }
        "nw/process-list" => {
            return Some(failure(task, "invalid_arguments", "usage: nw/process-list"));
        }
        "nw/process-kill" => {
            let [pid] = task.args.as_slice() else {
                return Some(failure(
                    task,
                    "invalid_arguments",
                    "usage: nw/process-kill <pid>",
                ));
            };
            let Ok(pid) = pid.parse::<u32>() else {
                return Some(failure(
                    task,
                    "invalid_pid",
                    "PID must be an unsigned integer",
                ));
            };
            kill(pid)
                .and_then(|result| {
                    serde_json::to_vec(&result).map_err(|_| ControlError::TerminateFailed { pid })
                })
                .map_err(|error| (error.code(), error.to_string()))
        }
        _ => return None,
    };

    Some(match output {
        Ok(stdout) => TaskResult {
            task_id: task.id,
            ok: true,
            stdout,
            stderr: Vec::new(),
            exit_code: 0,
        },
        Err((code, message)) => failure(task, code, message),
    })
}
