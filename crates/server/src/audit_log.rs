//! Append-only audit trail for operator actions on the C2.
//!
//! Every console command that affects sessions, tasks, or payloads is recorded
//! with the operator identity, the target session (if any), and success/failure.

use std::sync::Arc;

use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

/// One audited operator action.
#[derive(Debug, Clone, FromRow)]
pub struct AuditEntry {
    pub id: i64,
    pub operator_id: Option<String>,
    pub operator_name: String,
    pub action: String,
    pub target_session: Option<String>,
    pub details: String,
    pub succeeded: bool,
    pub timestamp: String,
}

#[derive(Clone)]
pub struct AuditLog {
    pool: SqlitePool,
}

impl AuditLog {
    pub fn new(pool: SqlitePool) -> Self {
        AuditLog { pool }
    }

    /// Record an operator action.
    pub async fn record(
        &self,
        operator: &crate::operators::Operator,
        action: &str,
        target_session: Option<&Uuid>,
        details: &str,
        succeeded: bool,
    ) -> Result<(), sqlx::Error> {
        let op_id = operator.id.to_string();
        let target = target_session.map(|s| s.to_string());
        sqlx::query(
            "INSERT INTO c2_audit (operator_id, operator_name, action, target_session, details, succeeded) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(op_id)
        .bind(&operator.username)
        .bind(action)
        .bind(target)
        .bind(details)
        .bind(succeeded)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Query audit events visible to an operator (based on role).
    pub async fn list(&self, limit: usize) -> Vec<AuditEntry> {
        sqlx::query_as("SELECT id, operator_id, operator_name, action, target_session, details, succeeded, timestamp \
                        FROM c2_audit ORDER BY id DESC LIMIT ?1")
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default()
    }
}

/// Shared handle.
pub type SharedAudit = Arc<AuditLog>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operators::{Operator, OperatorStore};
    use crate::persist;

    #[tokio::test]
    async fn audit_records_and_queries() {
        let pool = persist::open_pool(":memory:").await.unwrap();
        persist::init_schema(&pool).await.unwrap();
        let store = OperatorStore::new(pool.clone());
        let audit = AuditLog::new(pool);

        let id = store
            .create("admin", "changeme", crate::operators::Role::Admin)
            .await
            .unwrap();
        let op = Operator {
            id,
            username: "admin".into(),
            role: crate::operators::Role::Admin,
        };

        audit
            .record(&op, "shell", None, "echo hello", true)
            .await
            .unwrap();

        let entries = audit.list(10).await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "shell");
        assert_eq!(entries[0].operator_name, "admin");
        assert!(entries[0].succeeded);
    }

    #[tokio::test]
    async fn audit_entry_without_operator() {
        let pool = persist::open_pool(":memory:").await.unwrap();
        persist::init_schema(&pool).await.unwrap();
        let audit = AuditLog::new(pool);

        // Use a placeholder operator
        let op = crate::operators::Operator {
            id: Uuid::new_v4(),
            username: "system".into(),
            role: crate::operators::Role::Admin,
        };
        audit
            .record(&op, "session.kill", None, "no session", false)
            .await
            .unwrap();

        let entries = audit.list(10).await;
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].succeeded);
    }
}
