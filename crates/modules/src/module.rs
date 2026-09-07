use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Operator role — mirrors `nw_server::operators::Role`. Duplicated here
/// (rather than depending on nw-server) to keep the modules crate's dependency
/// graph acyclic: modules depends on profile only, not server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Admin,
    Operator,
    Viewer,
}

impl Role {
    /// True if `self` is at or above `required` in the hierarchy (admin > operator > viewer).
    pub fn allows(self, required: Self) -> bool {
        match (self, required) {
            (Role::Admin, _) => true,
            (Role::Operator, Role::Operator | Role::Viewer) => true,
            (Role::Operator, Role::Admin) => false,
            (Role::Viewer, Role::Viewer) => true,
            (Role::Viewer, _) => false,
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::Admin => write!(f, "admin"),
            Role::Operator => write!(f, "operator"),
            Role::Viewer => write!(f, "viewer"),
        }
    }
}

/// Structured output from a module execution. The `data` field is a free-form
/// JSON value so each module can declare its own schema without a shared type,
/// while `ok`/`error` give a uniform success/failure signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleOutput {
    pub module: String,
    pub task_id: Uuid,
    pub ok: bool,
    pub data: serde_json::Value,
    pub error: Option<String>,
}

/// A module's execution result on the implant side: the raw bytes (stdout)
/// plus a structured output envelope that gets deserialized by the server.
#[derive(Debug, Clone, Default)]
pub struct ModuleResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
    pub output: Option<ModuleOutput>,
}

impl ModuleResult {
    pub fn ok(module: &str, task_id: Uuid, data: serde_json::Value) -> Self {
        ModuleResult {
            stdout: serde_json::to_vec(&data).unwrap_or_default(),
            stderr: Vec::new(),
            exit_code: 0,
            output: Some(ModuleOutput {
                module: module.to_string(),
                task_id,
                ok: true,
                data,
                error: None,
            }),
        }
    }

    pub fn err(module: &str, task_id: Uuid, error: impl std::fmt::Display) -> Self {
        let msg = error.to_string();
        ModuleResult {
            stdout: Vec::new(),
            stderr: msg.as_bytes().to_vec(),
            exit_code: -1,
            output: Some(ModuleOutput {
                module: module.to_string(),
                task_id,
                ok: false,
                data: serde_json::Value::Null,
                error: Some(msg),
            }),
        }
    }
}

/// A post-exploitation module. Implemented by `impl Module` and registered
/// with the `Registry`.
///
/// Modules are identified by their `nw/<name>` command string so the console
/// dispatcher and the implant runtime can route `nw/<name> <args...>` to the
/// right handler.
pub trait Module: Send + Sync {
    /// The `nw/<name>` command that triggers this module.
    fn name(&self) -> &str;

    /// Human-readable description for `help` and the module catalog.
    fn description(&self) -> &str;

    /// Argument spec, e.g. `["path", "--format=table"]`.
    fn args(&self) -> &[&str];

    /// Required operator role to run this module.
    fn required_role(&self) -> Role {
        Role::Operator
    }

    /// Execute the module on the implant side and return structured output.
    fn run(&self, args: &[String]) -> ModuleResult;
}
