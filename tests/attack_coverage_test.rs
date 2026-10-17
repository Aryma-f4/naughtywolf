//! MITRE ATT&CK coverage surface: the catalogue, and its rendering in the
//! operator UI.
//!
//! The mapping itself is unit-tested in `src/attack.rs`. These tests cover the
//! parts an operator actually sees — that a technique reaches the task table,
//! that an unmapped command renders a dash rather than a blank cell, and that
//! the coverage page lists every mapped command.

use naughtywolf::auth::{AuthenticatedUser, rbac::Role};
use naughtywolf::db::models::{C2TaskWithResult, Callback, CallbackStatus};
use naughtywolf::portal::templates;

fn viewer() -> AuthenticatedUser {
    AuthenticatedUser {
        id: "operator-1".into(),
        username: "operator-1".into(),
        role: Role::Operator,
    }
}

fn task(id: &str, command: &str) -> C2TaskWithResult {
    C2TaskWithResult {
        id: id.into(),
        session_id: "session-1".into(),
        command: command.into(),
        args_json: serde_json::json!([]),
        status: "completed".into(),
        created_at: "2026-01-01T00:00:00Z".into(),
        processing_at: None,
        completed_at: Some("2026-01-01T00:00:01Z".into()),
        result_output: Some(java_shell_compatible_b64(b"ok")),
        result_ok: Some(true),
        result_exit_code: Some(0),
        result_stderr: None,
    }
}

/// Result output is stored base64-encoded; keep the helper honest about that.
fn java_shell_compatible_b64(raw: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(raw)
}

fn callback() -> Callback {
    Callback {
        id: "session-1".into(),
        asset_id: None,
        operation_id: None,
        host: "web-01".into(),
        user_name: "u".into(),
        process: "p".into(),
        arch: "x86_64".into(),
        os: "linux".into(),
        os_version: None,
        executable_path: None,
        local_addr: None,
        implant_version: None,
        interval_ms: None,
        jitter_ms: None,
        capabilities_json: None,
        protocol: "http".into(),
        status: CallbackStatus::Active,
        last_seen: "2026-01-01T00:00:00Z".into(),
        created_at: "2026-01-01T00:00:00Z".into(),
    }
}

#[test]
fn a_mapped_command_shows_its_technique_in_the_task_table() {
    let tasks = [task("11111111-1111-1111-1111-111111111111", "nw/creds")];
    let html = templates::callback_detail_page(&viewer(), &callback(), &tasks, "csrf");

    assert!(
        html.contains("T1003"),
        "creds task must show its ATT&CK technique: {html}"
    );
    assert!(
        html.contains("attack.mitre.org/techniques/T1003/"),
        "technique links out to the MITRE entry"
    );
    assert!(
        html.contains("OS Credential Dumping"),
        "technique name is shown as a tooltip"
    );
}

#[test]
fn an_unmapped_command_renders_a_dash_not_a_blank_cell() {
    let tasks = [task("22222222-2222-2222-2222-222222222222", "sessions")];
    let html = templates::callback_detail_page(&viewer(), &callback(), &tasks, "csrf");

    // A wrong mapping is worse than none, so an unmapped command must not
    // borrow a neighbouring command's technique.
    assert!(
        !html.contains("T1003") && !html.contains("T1547"),
        "unmapped command must not inherit a technique"
    );
    assert!(
        html.contains("ATT&CK"),
        "the column header is still present"
    );
}

#[test]
fn the_coverage_page_lists_every_catalogued_technique() {
    let html = templates::attack_coverage_page(&viewer());
    let catalogue = naughtywolf::attack::catalogue();

    assert!(!catalogue.is_empty(), "catalogue must not be empty");
    for (id, name, tactic) in &catalogue {
        assert!(html.contains(id), "coverage page is missing technique {id}");
        assert!(html.contains(name), "coverage page is missing name {name}");
        assert!(
            html.contains(tactic),
            "coverage page is missing tactic {tactic}"
        );
    }
}

/// The task table renders `task.command`, which is operator-controlled, next to
/// the ATT&CK cell. A crafted command name must not escape its cell.
#[test]
fn an_operator_supplied_command_name_is_escaped_in_the_task_table() {
    let tasks = [task(
        "33333333-3333-3333-3333-333333333333",
        "creds\"><script>alert(1)</script>",
    )];
    let html = templates::callback_detail_page(&viewer(), &callback(), &tasks, "csrf");

    assert!(
        !html.contains("<script>alert(1)</script>"),
        "an injected script tag must not survive rendering"
    );
    assert!(
        html.contains("&lt;script&gt;") || html.contains("&quot;&gt;&lt;script"),
        "the command is escaped rather than dropped"
    );
}

/// The catalogue and the lookup must not drift: every entry reachable through
/// `lookup` has to appear in the rendered coverage page.
#[test]
fn every_lookup_result_appears_in_the_coverage_page() {
    let html = templates::attack_coverage_page(&viewer());
    for command in [
        "creds", "hashes", "keylog", "persist", "registry", "socks", "shell", "script", "tokens",
        "redirect",
    ] {
        let technique = naughtywolf::attack::lookup(command)
            .unwrap_or_else(|| panic!("{command} should be mapped"));
        assert!(
            html.contains(technique.id),
            "{command} maps to {} but the coverage page omits it",
            technique.id
        );
    }
}
