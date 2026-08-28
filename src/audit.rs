/// Immutable data captured whenever a domain change is auditable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub actor_id: Option<String>,
    pub operation_id: Option<String>,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub parameter_summary: Option<String>,
    pub outcome: String,
    pub correlation_id: String,
}

impl AuditEntry {
    pub fn new(
        actor_id: impl Into<String>,
        action: impl Into<String>,
        target_type: impl Into<String>,
        target_id: impl Into<String>,
        outcome: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Self {
        Self {
            actor_id: Some(actor_id.into()),
            operation_id: None,
            action: action.into(),
            target_type: target_type.into(),
            target_id: Some(target_id.into()),
            parameter_summary: None,
            outcome: outcome.into(),
            correlation_id: correlation_id.into(),
        }
    }

    pub fn for_operation(mut self, operation_id: impl Into<String>) -> Self {
        self.operation_id = Some(operation_id.into());
        self
    }
}
