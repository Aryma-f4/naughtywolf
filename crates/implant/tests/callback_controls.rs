use std::{
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use nw_profile::control::{ControlError, ProcessEntry, ProcessListV1};
use nw_profile::msgs::Task;
use uuid::Uuid;

#[test]
fn process_contract_round_trips_nullable_platform_fields() {
    let fixture = r#"{
        "schema":"nw.process-list.v1",
        "captured_at":"2026-09-10T12:34:56Z",
        "processes":[{
            "pid":42,
            "parent_pid":1,
            "name":"worker",
            "executable":null,
            "user":null,
            "architecture":null,
            "cpu_percent":1.25,
            "memory_bytes":4096,
            "started_at":null
        }]
    }"#;

    let snapshot: ProcessListV1 = serde_json::from_str(fixture).expect("portable process fixture");
    assert_eq!(snapshot.schema, "nw.process-list.v1");
    assert_eq!(snapshot.captured_at, "2026-09-10T12:34:56Z");
    assert_eq!(
        snapshot.processes,
        vec![ProcessEntry {
            pid: 42,
            parent_pid: Some(1),
            name: "worker".to_owned(),
            executable: None,
            user: None,
            architecture: None,
            cpu_percent: 1.25,
            memory_bytes: 4096,
            started_at: None,
        }]
    );

    let encoded = serde_json::to_value(&snapshot).expect("serialize process snapshot");
    assert!(encoded["processes"][0]["pid"].is_u64());
    assert!(encoded["processes"][0]["parent_pid"].is_u64());
    assert!(encoded["processes"][0]["memory_bytes"].is_u64());
    assert!(encoded["processes"][0]["cpu_percent"].is_f64());
    assert!(encoded["processes"][0]["executable"].is_null());
}

#[test]
fn process_control_errors_have_stable_codes() {
    assert_eq!(
        ControlError::NoSuchProcess { pid: 7 }.code(),
        "no_such_process"
    );
    assert_eq!(
        ControlError::PermissionDenied { pid: 7 }.code(),
        "permission_denied"
    );
    assert_eq!(
        ControlError::TerminateFailed { pid: 7 }.code(),
        "terminate_failed"
    );
}

#[test]
fn process_list_contains_the_current_test_process() {
    let snapshot = nw_implant::processes::list();

    assert_eq!(snapshot.schema, "nw.process-list.v1");
    assert!(chrono::DateTime::parse_from_rfc3339(&snapshot.captured_at).is_ok());
    assert!(
        snapshot
            .processes
            .iter()
            .any(|process| process.pid == std::process::id())
    );
}

#[test]
fn process_dispatch_accepts_only_exact_structured_command_names() {
    let lookalike = Task {
        id: Uuid::new_v4(),
        command: "nw/process-list-extra".to_owned(),
        args: Vec::new(),
        timeout_ms: 1_000,
    };
    assert!(nw_implant::processes::execute(&lookalike).is_none());

    let list = Task {
        id: Uuid::new_v4(),
        command: "nw/process-list".to_owned(),
        args: Vec::new(),
        timeout_ms: 1_000,
    };
    let result = nw_implant::processes::execute(&list).expect("structured process handler");
    assert!(result.ok);
    let snapshot: ProcessListV1 =
        serde_json::from_slice(&result.stdout).expect("structured process result");
    assert_eq!(snapshot.schema, "nw.process-list.v1");
}

struct DedicatedChild(Child);

impl Drop for DedicatedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn process_kill_terminates_only_its_dedicated_child() {
    let current_test_pid = std::process::id();
    let executable = std::env::current_exe().expect("current integration test executable");
    let child = Command::new(executable)
        .arg("--exact")
        .arg("process_kill_dedicated_child_fixture")
        .arg("--nocapture")
        .env("NW_PROCESS_KILL_CHILD", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn dedicated kill-test child");
    let child_pid = child.id();
    let mut child = DedicatedChild(child);
    assert_ne!(child_pid, current_test_pid);
    thread::sleep(Duration::from_millis(250));

    let result = nw_implant::processes::kill(child_pid).expect("terminate dedicated child");
    assert_eq!(result.pid, child_pid);
    assert!(result.terminated);
    assert!(
        child
            .0
            .try_wait()
            .expect("inspect terminated dedicated child")
            .is_some(),
        "kill must not return before the dedicated child exits"
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child
            .0
            .try_wait()
            .expect("inspect dedicated child")
            .is_some()
        {
            break;
        }
        assert!(Instant::now() < deadline, "dedicated child did not exit");
        thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(std::process::id(), current_test_pid);
}

#[test]
fn process_kill_dedicated_child_fixture() {
    if std::env::var_os("NW_PROCESS_KILL_CHILD").is_some() {
        thread::sleep(Duration::from_secs(120));
    }
}
