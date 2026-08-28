use std::time::Duration;

use crate::auth::rbac::Role;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionKind {
    InProcess,
}

#[derive(Debug, Clone, Copy)]
pub struct CheckDefinition {
    pub id: &'static str,
    pub label: &'static str,
    pub required_role: Role,
    pub timeout: Duration,
    pub max_output_bytes: usize,
    pub builtin: bool,
    pub mutates_target: bool,
    pub execution_kind: ExecutionKind,
}

const CATALOG: [CheckDefinition; 1] = [CheckDefinition {
    id: "asset-record-review",
    label: "Asset record review",
    required_role: Role::Operator,
    timeout: Duration::from_secs(5),
    max_output_bytes: 4_096,
    builtin: true,
    mutates_target: false,
    execution_kind: ExecutionKind::InProcess,
}];

pub fn catalog() -> &'static [CheckDefinition] {
    &CATALOG
}
