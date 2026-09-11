use std::{
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use nw_profile::control::{
    ControlError, FileControlError, FileEntry, FileListV1, FileMutationV1, ProcessEntry,
    ProcessListV1,
};
use nw_profile::msgs::Task;
use std::fs;
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

#[test]
fn filesystem_list_and_stat_report_absolute_utf8_and_size_fields() {
    let tree = tempfile::tempdir().expect("fresh filesystem fixture");
    let directory = tree.path().join("documents");
    fs::create_dir(&directory).expect("fixture directory");
    let utf8_file = directory.join("数据.txt");
    let empty_file = directory.join("empty.bin");
    fs::write(&utf8_file, b"wolf").expect("nonempty fixture file");
    fs::write(&empty_file, []).expect("empty fixture file");

    let snapshot =
        nw_implant::filesystem::list(directory.to_str().unwrap()).expect("list temporary fixture");

    assert_eq!(snapshot.schema, "nw.fs-list.v1");
    assert!(snapshot.path.starts_with(tree.path().to_str().unwrap()));
    assert_eq!(snapshot.entries.len(), 2);
    let utf8 = snapshot
        .entries
        .iter()
        .find(|entry| entry.name == "数据.txt")
        .expect("UTF-8 entry");
    assert_eq!(utf8.kind, "file");
    assert_eq!(utf8.size, 4);
    assert_eq!(utf8.path, utf8_file.to_str().unwrap());
    #[cfg(unix)]
    assert!(utf8.owner.is_some(), "Unix owner id should be reported");
    let empty =
        nw_implant::filesystem::stat(empty_file.to_str().unwrap()).expect("stat empty fixture");
    assert_eq!(empty.name, "empty.bin");
    assert_eq!(empty.kind, "file");
    assert_eq!(empty.size, 0);
    assert_eq!(empty.path, empty_file.to_str().unwrap());
}

#[test]
fn filesystem_mutations_stay_within_the_exact_temporary_fixture_subtree() {
    let tree = tempfile::tempdir().expect("fresh filesystem fixture");
    let source = tree.path().join("source");
    let moved = tree.path().join("moved");
    let sibling = tree.path().join("sibling-marker.txt");
    let nested = source.join("nested.txt");
    fs::write(&sibling, b"keep").expect("sibling marker");

    let mkdir =
        nw_implant::filesystem::mkdir(source.to_str().unwrap()).expect("mkdir temporary fixture");
    assert_eq!(mkdir.schema, "nw.fs-mutation.v1");
    assert_eq!(mkdir.action, "mkdir");
    fs::write(&nested, b"nested").expect("nonempty directory fixture");
    let moved_result =
        nw_implant::filesystem::move_path(source.to_str().unwrap(), moved.to_str().unwrap())
            .expect("move temporary fixture");
    assert_eq!(moved_result.action, "move");
    assert_eq!(moved_result.destination.as_deref(), moved.to_str());
    assert!(!source.exists());
    assert!(moved.join("nested.txt").exists());

    let refusal = nw_implant::filesystem::delete(moved.to_str().unwrap(), false)
        .expect_err("nonrecursive delete must refuse a nonempty directory");
    assert_eq!(refusal, FileControlError::DirectoryNotEmpty);
    nw_implant::filesystem::delete(moved.to_str().unwrap(), true)
        .expect("recursive delete exact fixture subtree");

    assert!(!moved.exists());
    assert!(tree.path().exists(), "temporary fixture root must remain");
    assert_eq!(fs::read(&sibling).unwrap(), b"keep");
}

#[test]
fn filesystem_move_refuses_to_replace_an_existing_temporary_destination() {
    let tree = tempfile::tempdir().expect("fresh filesystem fixture");
    let source = tree.path().join("source.txt");
    let destination = tree.path().join("destination.txt");
    fs::write(&source, b"source").unwrap();
    fs::write(&destination, b"destination").unwrap();

    let error =
        nw_implant::filesystem::move_path(source.to_str().unwrap(), destination.to_str().unwrap())
            .expect_err("move must not overwrite an existing destination");

    assert_eq!(error, FileControlError::AlreadyExists);
    assert_eq!(fs::read(&source).unwrap(), b"source");
    assert_eq!(fs::read(&destination).unwrap(), b"destination");
}

#[test]
fn filesystem_dispatch_is_exact_and_validates_arguments_before_access() {
    let tree = tempfile::tempdir().expect("fresh filesystem fixture");
    let protected = tree.path().join("protected");
    fs::create_dir(&protected).unwrap();
    fs::write(protected.join("keep.txt"), b"keep").unwrap();
    let task = |command: &str, args: Vec<String>| Task {
        id: Uuid::new_v4(),
        command: command.to_owned(),
        args,
        timeout_ms: 1_000,
    };

    assert!(nw_implant::filesystem::execute(&task("nw/fs-list-extra", vec![])).is_none());
    for invalid in [
        task("nw/fs-list", vec![]),
        task(
            "nw/fs-delete",
            vec![protected.to_string_lossy().into_owned(), "TRUE".to_owned()],
        ),
        task(
            "nw/fs-delete",
            vec![protected.to_string_lossy().into_owned(), "1".to_owned()],
        ),
    ] {
        let result = nw_implant::filesystem::execute(&invalid).expect("filesystem handler");
        assert!(!result.ok);
        let error: serde_json::Value = serde_json::from_slice(&result.stderr).unwrap();
        assert_eq!(error["code"], "invalid_arguments");
    }
    assert!(protected.join("keep.txt").exists());
}

#[test]
fn filesystem_rejects_empty_and_nul_paths_with_stable_error_codes() {
    for path in ["", "bad\0path"] {
        let error = nw_implant::filesystem::stat(path).expect_err("invalid path");
        assert_eq!(error, FileControlError::InvalidPath);
        assert_eq!(error.code(), "invalid_path");
    }
    assert_eq!(FileControlError::NotFound.code(), "not_found");
    assert_eq!(
        FileControlError::PermissionDenied.code(),
        "permission_denied"
    );
    assert_eq!(FileControlError::AlreadyExists.code(), "already_exists");
    assert_eq!(
        FileControlError::DirectoryNotEmpty.code(),
        "directory_not_empty"
    );
    assert_eq!(FileControlError::IoFailure.code(), "io_failure");
}

#[test]
fn filesystem_contract_round_trips_portable_optional_fields() {
    let fixture = r#"{
        "schema":"nw.fs-list.v1",
        "captured_at":"2026-09-10T12:34:56Z",
        "path":"C:\\Temp",
        "entries":[{
            "name":"empty.txt","path":"C:\\Temp\\empty.txt","kind":"file","size":0,
            "modified_at":null,"permissions":null,"owner":null
        }]
    }"#;
    let snapshot: FileListV1 = serde_json::from_str(fixture).expect("portable file fixture");
    assert_eq!(
        snapshot.entries,
        vec![FileEntry {
            name: "empty.txt".to_owned(),
            path: r"C:\Temp\empty.txt".to_owned(),
            kind: "file".to_owned(),
            size: 0,
            modified_at: None,
            permissions: None,
            owner: None,
        }]
    );
    let mutation: FileMutationV1 = serde_json::from_value(serde_json::json!({
        "schema":"nw.fs-mutation.v1","action":"move","path":"/tmp/a",
        "destination":"/tmp/b","recursive":false,"completed_at":"2026-09-10T12:35:00Z"
    }))
    .unwrap();
    assert_eq!(mutation.destination.as_deref(), Some("/tmp/b"));
}
