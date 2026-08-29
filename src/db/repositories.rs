use serde_json::Value;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{
    audit::AuditEntry,
    auth::rbac::Role,
    db::models::{
        Asset, AssetStatus, AuditEvent, CheckRun, Evidence, Operation, OperationStatus, RunState,
    },
    error::AppError,
};

/// Password-free account record used by the local administrative portal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalUser {
    pub id: String,
    pub username: String,
    pub role: Role,
    pub disabled: bool,
    pub created_at: String,
}

#[derive(sqlx::FromRow)]
struct PortalUserRow {
    id: String,
    username: String,
    role: String,
    disabled: bool,
    created_at: String,
}

/// SQL-only access to persisted lab-platform records.
#[derive(Clone)]
pub struct Repository {
    pub pool: SqlitePool,
}

impl Repository {
    pub async fn list_users(&self) -> Result<Vec<PortalUser>, AppError> {
        let rows = sqlx::query_as::<_, PortalUserRow>(
            "SELECT id, username, role, disabled, created_at FROM users ORDER BY created_at, id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AppError::Internal)?;

        rows.into_iter()
            .map(|row| {
                Ok(PortalUser {
                    id: row.id,
                    username: row.username,
                    role: row.role.parse().map_err(|_| AppError::Internal)?,
                    disabled: row.disabled,
                    created_at: row.created_at,
                })
            })
            .collect()
    }

    pub async fn set_user_role_with_audit(
        &self,
        user_id: &str,
        role: Role,
        actor_id: &str,
        correlation_id: &str,
    ) -> Result<(), AppError> {
        if user_id == actor_id {
            return Err(AppError::Conflict(
                "administrators cannot change their own role".to_owned(),
            ));
        }

        let mut transaction = self.pool.begin().await.map_err(|_| AppError::Internal)?;
        let update = sqlx::query(
            "UPDATE users SET role = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE id = ?",
        )
        .bind(role.to_string())
        .bind(user_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;
        if update.rows_affected() == 0 {
            return Err(AppError::NotFound);
        }

        let entry = AuditEntry::new(
            actor_id,
            "user.role_changed",
            "user",
            user_id,
            "success",
            correlation_id,
        );
        insert_audit(&mut transaction, &entry).await?;
        transaction.commit().await.map_err(|_| AppError::Internal)
    }

    pub async fn set_user_disabled_with_audit(
        &self,
        user_id: &str,
        disabled: bool,
        actor_id: &str,
        correlation_id: &str,
    ) -> Result<(), AppError> {
        if user_id == actor_id {
            return Err(AppError::Conflict(
                "administrators cannot disable their own account".to_owned(),
            ));
        }

        let mut transaction = self.pool.begin().await.map_err(|_| AppError::Internal)?;
        let update = sqlx::query(
            "UPDATE users SET disabled = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE id = ?",
        )
        .bind(disabled)
        .bind(user_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;
        if update.rows_affected() == 0 {
            return Err(AppError::NotFound);
        }

        let entry = AuditEntry::new(
            actor_id,
            "user.disabled",
            "user",
            user_id,
            "success",
            correlation_id,
        );
        insert_audit(&mut transaction, &entry).await?;
        transaction.commit().await.map_err(|_| AppError::Internal)
    }

    pub async fn create_operation(&self, name: &str, purpose: &str) -> Result<Operation, AppError> {
        let id = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO operations (id, name, purpose) VALUES (?, ?, ?)")
            .bind(&id)
            .bind(name)
            .bind(purpose)
            .execute(&self.pool)
            .await
            .map_err(|_| AppError::Internal)?;

        sqlx::query_as::<_, Operation>("SELECT * FROM operations WHERE id = ?")
            .bind(&id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| AppError::Internal)
    }

    pub async fn create_operation_with_audit(
        &self,
        name: &str,
        purpose: &str,
        actor_id: &str,
        correlation_id: &str,
    ) -> Result<Operation, AppError> {
        let id = Uuid::new_v4().to_string();
        let mut transaction = self.pool.begin().await.map_err(|_| AppError::Internal)?;
        sqlx::query("INSERT INTO operations (id, name, purpose) VALUES (?, ?, ?)")
            .bind(&id)
            .bind(name)
            .bind(purpose)
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::Internal)?;

        let entry = AuditEntry::new(
            actor_id,
            "operation.created",
            "operation",
            &id,
            "success",
            correlation_id,
        )
        .for_operation(&id);
        insert_audit(&mut transaction, &entry).await?;
        transaction.commit().await.map_err(|_| AppError::Internal)?;

        self.find_operation(&id).await?.ok_or(AppError::NotFound)
    }

    pub async fn create_asset(
        &self,
        operation_id: &str,
        name: &str,
        kind: &str,
        owner: &str,
        address: &str,
    ) -> Result<Asset, AppError> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO assets (id, operation_id, name, kind, owner, address) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(operation_id)
        .bind(name)
        .bind(kind)
        .bind(owner)
        .bind(address)
        .execute(&self.pool)
        .await
        .map_err(|_| AppError::Internal)?;

        sqlx::query_as::<_, Asset>("SELECT * FROM assets WHERE id = ?")
            .bind(&id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| AppError::Internal)
    }

    pub async fn create_asset_with_audit(
        &self,
        operation_id: &str,
        name: &str,
        kind: &str,
        owner: &str,
        address: &str,
        actor_id: &str,
        correlation_id: &str,
    ) -> Result<Asset, AppError> {
        let id = Uuid::new_v4().to_string();
        let mut transaction = self.pool.begin().await.map_err(|_| AppError::Internal)?;
        sqlx::query(
            "INSERT INTO assets (id, operation_id, name, kind, owner, address) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(operation_id)
        .bind(name)
        .bind(kind)
        .bind(owner)
        .bind(address)
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;

        let entry = AuditEntry::new(
            actor_id,
            "asset.created",
            "asset",
            &id,
            "success",
            correlation_id,
        )
        .for_operation(operation_id);
        insert_audit(&mut transaction, &entry).await?;
        transaction.commit().await.map_err(|_| AppError::Internal)?;

        self.find_asset(&id).await?.ok_or(AppError::NotFound)
    }

    pub async fn list_assets(&self, operation_id: &str) -> Result<Vec<Asset>, AppError> {
        sqlx::query_as::<_, Asset>("SELECT * FROM assets WHERE operation_id = ? ORDER BY name, id")
            .bind(operation_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| AppError::Internal)
    }

    pub async fn list_operations_visible_to(
        &self,
        user_id: &str,
        is_admin: bool,
    ) -> Result<Vec<Operation>, AppError> {
        let query = if is_admin {
            sqlx::query_as::<_, Operation>("SELECT * FROM operations ORDER BY updated_at DESC, id")
                .fetch_all(&self.pool)
                .await
        } else {
            sqlx::query_as::<_, Operation>(
                "SELECT operations.* FROM operations \
                 JOIN operation_members ON operation_members.operation_id = operations.id \
                 WHERE operation_members.user_id = ? \
                 ORDER BY operations.updated_at DESC, operations.id",
            )
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
        };
        query.map_err(|_| AppError::Internal)
    }

    pub async fn list_assets_visible_to(
        &self,
        user_id: &str,
        is_admin: bool,
    ) -> Result<Vec<Asset>, AppError> {
        let query = if is_admin {
            sqlx::query_as::<_, Asset>("SELECT * FROM assets ORDER BY updated_at DESC, id")
                .fetch_all(&self.pool)
                .await
        } else {
            sqlx::query_as::<_, Asset>(
                "SELECT assets.* FROM assets \
                 JOIN operation_members ON operation_members.operation_id = assets.operation_id \
                 WHERE operation_members.user_id = ? \
                 ORDER BY assets.updated_at DESC, assets.id",
            )
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
        };
        query.map_err(|_| AppError::Internal)
    }

    pub async fn list_check_runs_visible_to(
        &self,
        user_id: &str,
        is_admin: bool,
    ) -> Result<Vec<CheckRun>, AppError> {
        let query = if is_admin {
            sqlx::query_as::<_, CheckRun>("SELECT * FROM check_runs ORDER BY created_at DESC, id")
                .fetch_all(&self.pool)
                .await
        } else {
            sqlx::query_as::<_, CheckRun>(
                "SELECT check_runs.* FROM check_runs \
                 JOIN operation_members ON operation_members.operation_id = check_runs.operation_id \
                 WHERE operation_members.user_id = ? \
                 ORDER BY check_runs.created_at DESC, check_runs.id",
            )
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
        };
        query.map_err(|_| AppError::Internal)
    }

    pub async fn list_evidence_visible_to(
        &self,
        user_id: &str,
        is_admin: bool,
    ) -> Result<Vec<Evidence>, AppError> {
        let query = if is_admin {
            sqlx::query_as::<_, Evidence>("SELECT * FROM evidence ORDER BY created_at DESC, id")
                .fetch_all(&self.pool)
                .await
        } else {
            sqlx::query_as::<_, Evidence>(
                "SELECT evidence.* FROM evidence \
                 JOIN check_runs ON check_runs.id = evidence.check_run_id \
                 JOIN operation_members ON operation_members.operation_id = check_runs.operation_id \
                 WHERE operation_members.user_id = ? \
                 ORDER BY evidence.created_at DESC, evidence.id",
            )
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
        };
        query.map_err(|_| AppError::Internal)
    }

    pub async fn find_evidence_visible_to(
        &self,
        evidence_id: &str,
        user_id: &str,
        is_admin: bool,
    ) -> Result<Option<Evidence>, AppError> {
        let query = if is_admin {
            sqlx::query_as::<_, Evidence>("SELECT * FROM evidence WHERE id = ?")
                .bind(evidence_id)
                .fetch_optional(&self.pool)
                .await
        } else {
            sqlx::query_as::<_, Evidence>(
                "SELECT evidence.* FROM evidence \
                 JOIN check_runs ON check_runs.id = evidence.check_run_id \
                 JOIN operation_members ON operation_members.operation_id = check_runs.operation_id \
                 WHERE evidence.id = ? AND operation_members.user_id = ?",
            )
            .bind(evidence_id)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
        };
        query.map_err(|_| AppError::Internal)
    }

    pub async fn list_audit_events_visible_to(
        &self,
        user_id: &str,
        is_admin: bool,
    ) -> Result<Vec<AuditEvent>, AppError> {
        let query = if is_admin {
            sqlx::query_as::<_, AuditEvent>(
                "SELECT * FROM audit_events ORDER BY created_at DESC, id",
            )
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as::<_, AuditEvent>(
                "SELECT audit_events.* FROM audit_events \
                 JOIN operation_members ON operation_members.operation_id = audit_events.operation_id \
                 WHERE operation_members.user_id = ? \
                 ORDER BY audit_events.created_at DESC, audit_events.id",
            )
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
        };
        query.map_err(|_| AppError::Internal)
    }

    pub async fn count_operations_visible_to(
        &self,
        user_id: &str,
        is_admin: bool,
    ) -> Result<i64, AppError> {
        let query = if is_admin {
            sqlx::query_scalar("SELECT COUNT(*) FROM operations")
                .fetch_one(&self.pool)
                .await
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM operations \
                 JOIN operation_members ON operation_members.operation_id = operations.id \
                 WHERE operation_members.user_id = ?",
            )
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
        };
        query.map_err(|_| AppError::Internal)
    }

    pub async fn count_assets_visible_to(
        &self,
        user_id: &str,
        is_admin: bool,
    ) -> Result<i64, AppError> {
        let query = if is_admin {
            sqlx::query_scalar("SELECT COUNT(*) FROM assets")
                .fetch_one(&self.pool)
                .await
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM assets \
                 JOIN operation_members ON operation_members.operation_id = assets.operation_id \
                 WHERE operation_members.user_id = ?",
            )
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
        };
        query.map_err(|_| AppError::Internal)
    }

    pub async fn count_check_runs_visible_to(
        &self,
        user_id: &str,
        is_admin: bool,
    ) -> Result<i64, AppError> {
        let query = if is_admin {
            sqlx::query_scalar("SELECT COUNT(*) FROM check_runs")
                .fetch_one(&self.pool)
                .await
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM check_runs \
                 JOIN operation_members ON operation_members.operation_id = check_runs.operation_id \
                 WHERE operation_members.user_id = ?",
            )
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
        };
        query.map_err(|_| AppError::Internal)
    }

    pub async fn count_evidence_visible_to(
        &self,
        user_id: &str,
        is_admin: bool,
    ) -> Result<i64, AppError> {
        let query = if is_admin {
            sqlx::query_scalar("SELECT COUNT(*) FROM evidence")
                .fetch_one(&self.pool)
                .await
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM evidence \
                 JOIN check_runs ON check_runs.id = evidence.check_run_id \
                 JOIN operation_members ON operation_members.operation_id = check_runs.operation_id \
                 WHERE operation_members.user_id = ?",
            )
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
        };
        query.map_err(|_| AppError::Internal)
    }

    pub async fn count_audit_events_visible_to(
        &self,
        user_id: &str,
        is_admin: bool,
    ) -> Result<i64, AppError> {
        let query = if is_admin {
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
                .fetch_one(&self.pool)
                .await
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM audit_events \
                 JOIN operation_members ON operation_members.operation_id = audit_events.operation_id \
                 WHERE operation_members.user_id = ?",
            )
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
        };
        query.map_err(|_| AppError::Internal)
    }

    pub async fn find_operation(&self, operation_id: &str) -> Result<Option<Operation>, AppError> {
        sqlx::query_as::<_, Operation>("SELECT * FROM operations WHERE id = ?")
            .bind(operation_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| AppError::Internal)
    }

    pub async fn find_asset(&self, asset_id: &str) -> Result<Option<Asset>, AppError> {
        sqlx::query_as::<_, Asset>("SELECT * FROM assets WHERE id = ?")
            .bind(asset_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| AppError::Internal)
    }

    pub async fn is_operation_member(
        &self,
        operation_id: &str,
        user_id: &str,
    ) -> Result<bool, AppError> {
        sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM operation_members WHERE operation_id = ? AND user_id = ?)",
        )
        .bind(operation_id)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map(|exists| exists != 0)
        .map_err(|_| AppError::Internal)
    }

    pub async fn require_active_operation(&self, operation_id: &str) -> Result<(), AppError> {
        let operation = self
            .find_operation(operation_id)
            .await?
            .ok_or(AppError::NotFound)?;
        if operation.status != OperationStatus::Active {
            return Err(AppError::Conflict(
                "new runs require an active operation".to_owned(),
            ));
        }
        Ok(())
    }

    pub async fn require_active_asset(&self, asset_id: &str) -> Result<(), AppError> {
        let asset = self.find_asset(asset_id).await?.ok_or(AppError::NotFound)?;
        if asset.status != AssetStatus::Active {
            return Err(AppError::Conflict(
                "new runs require an active asset".to_owned(),
            ));
        }
        Ok(())
    }

    pub async fn add_member(&self, operation_id: &str, user_id: &str) -> Result<(), AppError> {
        sqlx::query("INSERT INTO operation_members (operation_id, user_id) VALUES (?, ?)")
            .bind(operation_id)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(|_| AppError::Internal)?;
        Ok(())
    }

    pub async fn create_run(
        &self,
        check_id: &str,
        asset_id: &str,
        operation_id: &str,
        requested_by: Option<&str>,
        input_json: &Value,
    ) -> Result<CheckRun, AppError> {
        let id = Uuid::new_v4().to_string();
        let input_json = serde_json::to_string(input_json).map_err(|_| AppError::Internal)?;
        sqlx::query(
            "INSERT INTO check_runs \
             (id, check_id, asset_id, operation_id, requested_by, state, input_json) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(check_id)
        .bind(asset_id)
        .bind(operation_id)
        .bind(requested_by)
        .bind(RunState::Queued)
        .bind(input_json)
        .execute(&self.pool)
        .await
        .map_err(|_| AppError::Internal)?;

        sqlx::query_as::<_, CheckRun>("SELECT * FROM check_runs WHERE id = ?")
            .bind(&id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| AppError::Internal)
    }

    pub async fn ensure_builtin_check(
        &self,
        id: &str,
        name: &str,
        required_role: &str,
        timeout_seconds: i64,
        output_limit_bytes: i64,
    ) -> Result<(), AppError> {
        if output_limit_bytes < 2 {
            return Err(AppError::Validation(
                "catalog output limit must be at least 2 bytes".to_owned(),
            ));
        }
        sqlx::query(
            "INSERT INTO checks \
             (id, name, description, version, required_role, input_schema, result_schema, timeout_seconds, output_limit_bytes) \
             VALUES (?, ?, ?, '1', ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO NOTHING",
        )
        .bind(id)
        .bind(name)
        .bind("Built-in, non-destructive in-process check")
        .bind(required_role)
        .bind(r#"{"type":"object","additionalProperties":false}"#)
        .bind(r#"{"type":"object"}"#)
        .bind(timeout_seconds)
        .bind(output_limit_bytes)
        .execute(&self.pool)
        .await
        .map_err(|_| AppError::Internal)?;
        Ok(())
    }

    pub async fn output_limit_for_run(&self, run_id: &str) -> Result<i64, AppError> {
        sqlx::query_scalar(
            "SELECT checks.output_limit_bytes \
             FROM check_runs JOIN checks ON checks.id = check_runs.check_id \
             WHERE check_runs.id = ?",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AppError::Internal)?
        .ok_or(AppError::NotFound)
    }

    pub async fn find_evidence_for_run_at_path(
        &self,
        run_id: &str,
        storage_path: &str,
    ) -> Result<Option<Evidence>, AppError> {
        sqlx::query_as::<_, Evidence>(
            "SELECT * FROM evidence WHERE check_run_id = ? AND storage_path = ?",
        )
        .bind(run_id)
        .bind(storage_path)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AppError::Internal)
    }

    pub async fn create_evidence(
        &self,
        run_id: &str,
        storage_path: &str,
        content_type: &str,
        byte_len: i64,
        sha256: &str,
    ) -> Result<Evidence, AppError> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO evidence (id, check_run_id, storage_path, content_type, byte_len, sha256) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(run_id)
        .bind(storage_path)
        .bind(content_type)
        .bind(byte_len)
        .bind(sha256)
        .execute(&self.pool)
        .await
        .map_err(|_| AppError::Internal)?;

        sqlx::query_as::<_, Evidence>("SELECT * FROM evidence WHERE id = ?")
            .bind(&id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| AppError::Internal)
    }

    pub async fn count_evidence(&self) -> Result<i64, AppError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM evidence")
            .fetch_one(&self.pool)
            .await
            .map_err(|_| AppError::Internal)
    }

    pub async fn rename_operation_with_audit(
        &self,
        operation_id: &str,
        name: &str,
        actor_id: &str,
        correlation_id: &str,
    ) -> Result<Operation, AppError> {
        let mut transaction = self.pool.begin().await.map_err(|_| AppError::Internal)?;
        let update = sqlx::query(
            "UPDATE operations \
             SET name = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE id = ?",
        )
        .bind(name)
        .bind(operation_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;
        if update.rows_affected() == 0 {
            return Err(AppError::NotFound);
        }

        let entry = AuditEntry::new(
            actor_id,
            "operation.renamed",
            "operation",
            operation_id,
            "success",
            correlation_id,
        )
        .for_operation(operation_id);
        insert_audit(&mut transaction, &entry).await?;
        transaction.commit().await.map_err(|_| AppError::Internal)?;

        self.find_operation(operation_id)
            .await?
            .ok_or(AppError::NotFound)
    }

    pub async fn count_audit_events(&self) -> Result<i64, AppError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
            .fetch_one(&self.pool)
            .await
            .map_err(|_| AppError::Internal)
    }

    pub async fn start_run(&self, run_id: &str) -> Result<CheckRun, AppError> {
        let update = sqlx::query(
            "UPDATE check_runs \
             SET state = ?, started_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE id = ? AND state = ?",
        )
        .bind(RunState::Running)
        .bind(run_id)
        .bind(RunState::Queued)
        .execute(&self.pool)
        .await
        .map_err(|_| AppError::Internal)?;
        if update.rows_affected() == 0 {
            return Err(AppError::Conflict(
                "check run cannot transition to running".to_owned(),
            ));
        }

        sqlx::query_as::<_, CheckRun>("SELECT * FROM check_runs WHERE id = ?")
            .bind(run_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| AppError::Internal)
    }

    pub async fn finish_run(
        &self,
        run_id: &str,
        state: RunState,
        result_json: Option<&Value>,
        error_summary: Option<&str>,
        output_truncated: bool,
    ) -> Result<CheckRun, AppError> {
        if !matches!(
            state,
            RunState::Succeeded | RunState::Failed | RunState::Cancelled
        ) {
            return Err(AppError::Validation(
                "a check run must finish in a terminal state".to_owned(),
            ));
        }

        let existing_state =
            sqlx::query_scalar::<_, RunState>("SELECT state FROM check_runs WHERE id = ?")
                .bind(run_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|_| AppError::Internal)?
                .ok_or(AppError::NotFound)?;
        if existing_state == RunState::Queued {
            self.start_run(run_id).await?;
        } else if existing_state != RunState::Running {
            return Err(AppError::Conflict(
                "check run is already in a terminal state".to_owned(),
            ));
        }

        let result_json = result_json
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| AppError::Internal)?;
        let update = sqlx::query(
            "UPDATE check_runs \
             SET state = ?, result_json = ?, error_summary = ?, output_truncated = ?, \
                 finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE id = ? AND state = ?",
        )
        .bind(state)
        .bind(result_json)
        .bind(error_summary)
        .bind(output_truncated)
        .bind(run_id)
        .bind(RunState::Running)
        .execute(&self.pool)
        .await
        .map_err(|_| AppError::Internal)?;
        if update.rows_affected() == 0 {
            return Err(AppError::NotFound);
        }

        sqlx::query_as::<_, CheckRun>("SELECT * FROM check_runs WHERE id = ?")
            .bind(run_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| AppError::Internal)
    }
}

async fn insert_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    entry: &AuditEntry,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO audit_events \
         (id, actor_id, operation_id, action, target_type, target_id, parameter_summary, outcome, correlation_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&entry.actor_id)
    .bind(&entry.operation_id)
    .bind(&entry.action)
    .bind(&entry.target_type)
    .bind(&entry.target_id)
    .bind(&entry.parameter_summary)
    .bind(&entry.outcome)
    .bind(&entry.correlation_id)
    .execute(&mut **transaction)
    .await
    .map_err(|_| AppError::Internal)?;
    Ok(())
}
