use serde_json::Value;
use sqlx::SqlitePool;
use uuid::Uuid;

use nw_profile::msgs::Register;

use crate::{
    audit::AuditEntry,
    auth::rbac::Role,
    db::models::{
        Asset, AssetStatus, AuditEvent, Callback, CheckRun, EventRule, Evidence, InstalledService,
        Operation, OperationStatus, RunState,
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

    pub async fn list_callbacks_visible_to(
        &self,
        user_id: &str,
        is_admin: bool,
    ) -> Result<Vec<Callback>, AppError> {
        let query = if is_admin {
            sqlx::query_as::<_, Callback>("SELECT * FROM callbacks ORDER BY last_seen DESC, id")
                .fetch_all(&self.pool)
                .await
        } else {
            sqlx::query_as::<_, Callback>(
                "SELECT callbacks.* FROM callbacks \
                 JOIN operation_members ON operation_members.operation_id = callbacks.operation_id \
                 WHERE operation_members.user_id = ? \
                 ORDER BY callbacks.last_seen DESC, callbacks.id",
            )
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
        };
        query.map_err(|_| AppError::Internal)
    }

    /// Record or refresh a beacon identified by its assigned session id.
    pub async fn upsert_callback(
        &self,
        session_id: &str,
        hostname: &str,
        username: &str,
        os: &str,
        arch: &str,
        pid: u32,
        session_key: &str,
    ) -> Result<(), AppError> {
        self.upsert_callback_with_protocol(
            session_id,
            hostname,
            username,
            os,
            arch,
            pid,
            session_key,
            "http",
        )
        .await
    }

    pub async fn upsert_callback_with_protocol(
        &self,
        session_id: &str,
        hostname: &str,
        username: &str,
        os: &str,
        arch: &str,
        pid: u32,
        session_key: &str,
        protocol: &str,
    ) -> Result<(), AppError> {
        self.upsert_callback_record(
            session_id,
            hostname,
            username,
            os,
            arch,
            pid,
            "",
            session_key,
            protocol,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
    }

    /// Persist an implant's extended registration while retaining the legacy
    /// upsert entry points used by older integrations.
    pub async fn upsert_callback_registration(
        &self,
        session_id: &str,
        register: &Register,
        session_key: &str,
        protocol: &str,
    ) -> Result<(), AppError> {
        let local_addr = register
            .local_addr
            .as_deref()
            .or_else(|| (register.addr != "unknown").then_some(register.addr.as_str()));
        let capabilities = register
            .capabilities
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| AppError::Internal)?;

        self.upsert_callback_record(
            session_id,
            &register.hostname,
            &register.username,
            &register.os,
            &register.arch,
            register.pid,
            local_addr.unwrap_or(&register.addr),
            session_key,
            protocol,
            register.os_version.as_deref(),
            register.executable_path.as_deref(),
            local_addr,
            register.implant_version.as_deref(),
            register.interval_ms.map(u64_to_i64),
            register.jitter_ms.map(u64_to_i64),
            capabilities.as_ref(),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn upsert_callback_record(
        &self,
        session_id: &str,
        hostname: &str,
        username: &str,
        os: &str,
        arch: &str,
        pid: u32,
        addr: &str,
        session_key: &str,
        protocol: &str,
        os_version: Option<&str>,
        executable_path: Option<&str>,
        local_addr: Option<&str>,
        implant_version: Option<&str>,
        interval_ms: Option<i64>,
        jitter_ms: Option<i64>,
        capabilities_json: Option<&serde_json::Value>,
    ) -> Result<(), AppError> {
        let mut transaction = self.pool.begin().await.map_err(|_| AppError::Internal)?;
        let process = pid.to_string();
        sqlx::query(
            "INSERT INTO callbacks (id, host, user_name, os, arch, process, protocol, status, session_key, last_seen, \
                                     os_version, executable_path, local_addr, implant_version, interval_ms, jitter_ms, capabilities_json) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET \
             host = excluded.host, user_name = excluded.user_name, os = excluded.os, \
             arch = excluded.arch, process = excluded.process, protocol = excluded.protocol, status = 'active', \
             session_key = excluded.session_key, \
             os_version = excluded.os_version, executable_path = excluded.executable_path, \
             local_addr = excluded.local_addr, implant_version = excluded.implant_version, \
             interval_ms = excluded.interval_ms, jitter_ms = excluded.jitter_ms, \
             capabilities_json = excluded.capabilities_json, \
             last_seen = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        )
        .bind(session_id)
        .bind(hostname)
        .bind(username)
        .bind(os)
        .bind(arch)
        .bind(&process)
        .bind(protocol)
        .bind(session_key)
        .bind(os_version)
        .bind(executable_path)
        .bind(local_addr)
        .bind(implant_version)
        .bind(interval_ms)
        .bind(jitter_ms)
        .bind(capabilities_json)
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;
        sqlx::query(
            "INSERT INTO c2_sessions (id, hostname, username, os, arch, pid, addr, session_key, last_seen) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')) \
             ON CONFLICT(id) DO UPDATE SET hostname = excluded.hostname, username = excluded.username, \
             os = excluded.os, arch = excluded.arch, pid = excluded.pid, addr = excluded.addr, session_key = excluded.session_key, \
             last_seen = excluded.last_seen",
        )
        .bind(session_id)
        .bind(hostname)
        .bind(username)
        .bind(os)
        .bind(arch)
        .bind(pid as i64)
        .bind(addr)
        .bind(session_key.as_bytes())
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;
        transaction.commit().await.map_err(|_| AppError::Internal)?;
        Ok(())
    }

    /// Look up the per-session AES key (base64) for a callback id, for routing
    /// beacon polls to the right cipher. Returns None if unknown.
    pub async fn session_key_for(&self, session_id: &str) -> Result<Option<String>, AppError> {
        sqlx::query_scalar::<_, String>("SELECT session_key FROM callbacks WHERE id = ?")
            .bind(session_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| AppError::Internal)
    }

    /// Refresh a callback's last-seen timestamp on each beacon poll.
    pub async fn touch_callback(&self, session_id: &str) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE callbacks SET last_seen = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), status = 'active' \
             WHERE id = ?",
        )
        .bind(session_id)
        .execute(&self.pool)
        .await
        .map_err(|_| AppError::Internal)?;
        Ok(())
    }

    /// Enqueue a task for a session (Mythic-style: creates a task in 'pending' status).
    pub async fn enqueue_task(
        &self,
        session_id: &str,
        command: &str,
        args_json: &serde_json::Value,
        timeout_ms: u64,
    ) -> Result<String, AppError> {
        let task_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO c2_tasks (id, session_id, command, args_json, timeout_ms, status)
             VALUES (?, ?, ?, ?, ?, 'pending')",
        )
        .bind(&task_id)
        .bind(session_id)
        .bind(command)
        .bind(args_json)
        .bind(timeout_ms as i64)
        .execute(&self.pool)
        .await
        .map_err(|_| AppError::Internal)?;
        Ok(task_id)
    }

    /// Return work until its delivery is acknowledged by the owning callback.
    pub async fn tasks_for_delivery(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::db::models::C2Task>, AppError> {
        let mut transaction = self.pool.begin().await.map_err(|_| AppError::Internal)?;
        let rows: Vec<crate::db::models::C2Task> = sqlx::query_as(
            "SELECT * FROM c2_tasks WHERE session_id = ? AND status IN ('pending', 'delivered') \
             ORDER BY created_at, id",
        )
        .bind(session_id)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;
        sqlx::query(
            "UPDATE c2_tasks SET status = 'delivered', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE session_id = ? AND status = 'pending'",
        )
        .bind(session_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;
        transaction.commit().await.map_err(|_| AppError::Internal)?;
        Ok(rows)
    }

    /// Advance acknowledged deliveries without allowing cross-session ids.
    pub async fn acknowledge_tasks(&self, session_id: &str, ids: &[Uuid]) -> Result<(), AppError> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut transaction = self.pool.begin().await.map_err(|_| AppError::Internal)?;
        for id in ids {
            let owner: Option<String> =
                sqlx::query_scalar("SELECT session_id FROM c2_tasks WHERE id = ?")
                    .bind(id.to_string())
                    .fetch_optional(&mut *transaction)
                    .await
                    .map_err(|_| AppError::Internal)?;
            if owner.as_deref() != Some(session_id) {
                return Err(AppError::NotFound);
            }
        }
        for id in ids {
            sqlx::query(
                "UPDATE c2_tasks SET status = 'processing', \
                 processing_at = COALESCE(processing_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')), \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
                 WHERE id = ? AND session_id = ? AND status IN ('delivering', 'delivered')",
            )
            .bind(id.to_string())
            .bind(session_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::Internal)?;
        }
        transaction.commit().await.map_err(|_| AppError::Internal)
    }

    /// Backward-compatible name retained for callers outside the callback poll path.
    pub async fn fetch_pending_tasks(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::db::models::C2Task>, AppError> {
        self.tasks_for_delivery(session_id).await
    }

    /// Store a task result and mark the task as completed or error (Mythic-style).
    pub async fn store_task_result(
        &self,
        task_id: &str,
        ok: bool,
        stdout: &[u8],
        stderr: &[u8],
        exit_code: i32,
    ) -> Result<(), AppError> {
        let session_id: String = sqlx::query_scalar("SELECT session_id FROM c2_tasks WHERE id = ?")
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| AppError::Internal)?
            .ok_or(AppError::NotFound)?;
        self.store_task_result_for_session(&session_id, task_id, ok, stdout, stderr, exit_code)
            .await
            .and_then(|stored| {
                if stored {
                    Ok(())
                } else {
                    Err(AppError::NotFound)
                }
            })
    }

    /// Atomically store one owned result. Replays are acknowledged without a
    /// second result write or terminal audit entry.
    pub async fn store_task_result_for_session(
        &self,
        session_id: &str,
        task_id: &str,
        ok: bool,
        stdout: &[u8],
        stderr: &[u8],
        exit_code: i32,
    ) -> Result<bool, AppError> {
        use base64::Engine;

        let mut transaction = self.pool.begin().await.map_err(|_| AppError::Internal)?;
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT status, cancellation_requested_at FROM c2_tasks WHERE id = ? AND session_id = ?",
        )
        .bind(task_id)
        .bind(session_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;
        let Some((current, cancellation_requested_at)) = row else {
            return Ok(false);
        };
        if matches!(current.as_str(), "completed" | "error" | "cancelled") {
            transaction.commit().await.map_err(|_| AppError::Internal)?;
            return Ok(true);
        }

        let status = if ok {
            "completed"
        } else if cancellation_requested_at.is_some() {
            "cancelled"
        } else {
            "error"
        };
        sqlx::query(
            "INSERT INTO c2_task_results (task_id, ok, stdout, stderr, exit_code, completed_at) \
             VALUES (?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        )
        .bind(task_id)
        .bind(if ok { 1 } else { 0 })
        .bind(stdout)
        .bind(stderr)
        .bind(exit_code)
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;
        sqlx::query(
            "UPDATE c2_tasks SET status = ?, completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), \
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), result_output = ?, \
             result_ok = ?, result_exit_code = ? WHERE id = ? AND session_id = ?",
        )
        .bind(status)
        .bind(base64::engine::general_purpose::STANDARD.encode(stdout))
        .bind(if ok { 1 } else { 0 })
        .bind(exit_code)
        .bind(task_id)
        .bind(session_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;
        let action = match status {
            "completed" => "task_completed",
            "cancelled" => "task_cancelled",
            _ => "task_error",
        };
        sqlx::query(
            "INSERT INTO c2_audit (operator_id, operator_name, action, target_session, details, succeeded, timestamp) \
             VALUES ('', 'system', ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        )
        .bind(action)
        .bind(session_id)
        .bind(format!("task {task_id}"))
        .bind(if ok || status == "cancelled" { 1 } else { 0 })
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;
        transaction.commit().await.map_err(|_| AppError::Internal)?;
        Ok(true)
    }

    pub async fn request_task_cancellation(
        &self,
        session_id: &str,
        task_id: &str,
        operator_id: &str,
        operator_name: &str,
    ) -> Result<crate::db::models::TaskCancellation, AppError> {
        use crate::db::models::TaskCancellation;

        let mut transaction = self.pool.begin().await.map_err(|_| AppError::Internal)?;
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT status, cancellation_requested_at FROM c2_tasks WHERE id = ? AND session_id = ?",
        )
        .bind(task_id)
        .bind(session_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;
        let Some((status, cancellation_requested_at)) = row else {
            return Err(AppError::NotFound);
        };
        if matches!(status.as_str(), "completed" | "error" | "cancelled") {
            transaction.commit().await.map_err(|_| AppError::Internal)?;
            return Ok(TaskCancellation::AlreadyTerminal);
        }
        if status == "pending" {
            sqlx::query(
                "UPDATE c2_tasks SET status = 'cancelled', cancellation_requested_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), \
                 completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
                 WHERE id = ? AND session_id = ? AND status = 'pending'",
            )
            .bind(task_id)
            .bind(session_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::Internal)?;
            sqlx::query(
                "INSERT INTO c2_audit (operator_id, operator_name, action, target_session, details, succeeded, timestamp) \
                 VALUES (?, ?, 'task_cancelled', ?, ?, 1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            )
            .bind(operator_id)
            .bind(operator_name)
            .bind(session_id)
            .bind(format!("task {task_id}"))
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::Internal)?;
            transaction.commit().await.map_err(|_| AppError::Internal)?;
            return Ok(TaskCancellation::Cancelled);
        }

        if cancellation_requested_at.is_some() {
            let control_task_id: String = sqlx::query_scalar(
                "SELECT id FROM c2_tasks WHERE session_id = ? AND parent_task_id = ? AND command = 'nw/killtask' \
                 ORDER BY created_at, id LIMIT 1",
            )
            .bind(session_id)
            .bind(task_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| AppError::Internal)?;
            transaction.commit().await.map_err(|_| AppError::Internal)?;
            return Ok(TaskCancellation::Requested { control_task_id });
        }

        let control_task_id = Uuid::new_v4().to_string();
        sqlx::query(
            "UPDATE c2_tasks SET cancellation_requested_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), \
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ? AND session_id = ?",
        )
        .bind(task_id)
        .bind(session_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;
        sqlx::query(
            "INSERT INTO c2_tasks (id, session_id, command, args_json, timeout_ms, status, operator_id, parent_task_id) \
             VALUES (?, ?, 'nw/killtask', ?, 10000, 'pending', ?, ?)",
        )
        .bind(&control_task_id)
        .bind(session_id)
        .bind(serde_json::json!([task_id]))
        .bind(operator_id)
        .bind(task_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;
        sqlx::query(
            "INSERT INTO c2_audit (operator_id, operator_name, action, target_session, details, succeeded, timestamp) \
             VALUES (?, ?, 'task_cancellation_requested', ?, ?, 1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        )
        .bind(operator_id)
        .bind(operator_name)
        .bind(session_id)
        .bind(format!("task {task_id}"))
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;
        transaction.commit().await.map_err(|_| AppError::Internal)?;
        Ok(TaskCancellation::Requested { control_task_id })
    }

    /// List tasks for a session with their results (for the callback detail page).
    pub async fn list_tasks_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::db::models::C2TaskWithResult>, AppError> {
        sqlx::query_as::<_, crate::db::models::C2TaskWithResult>(
            "SELECT t.id, t.session_id, t.command, t.args_json, t.status, t.created_at,
                    t.processing_at, t.completed_at, t.result_output, t.result_ok,
                    r.exit_code AS result_exit_code,
                    r.stderr AS result_stderr
             FROM c2_tasks t
             LEFT JOIN c2_task_results r ON r.task_id = t.id
             WHERE t.session_id = ?
             ORDER BY t.created_at DESC, t.id DESC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| AppError::Internal)
    }

    /// Find a callback (session) by ID.
    pub async fn find_callback(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::db::models::Callback>, AppError> {
        sqlx::query_as("SELECT * FROM callbacks WHERE id = ?")
            .bind(session_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| AppError::Internal)
    }

    pub async fn list_event_rules(&self) -> Result<Vec<EventRule>, AppError> {
        sqlx::query_as::<_, EventRule>("SELECT * FROM event_rules ORDER BY created_at DESC, id")
            .fetch_all(&self.pool)
            .await
            .map_err(|_| AppError::Internal)
    }

    pub async fn create_event_rule(
        &self,
        name: &str,
        trigger: &str,
        command: &str,
        target: &str,
        actor_id: &str,
        correlation_id: &str,
    ) -> Result<EventRule, AppError> {
        let id = Uuid::new_v4().to_string();
        let mut transaction = self.pool.begin().await.map_err(|_| AppError::Internal)?;
        sqlx::query(
            "INSERT INTO event_rules (id, name, trigger, command, target, requested_by) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(name)
        .bind(trigger)
        .bind(command)
        .bind(target)
        .bind(actor_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;

        let entry = AuditEntry::new(
            actor_id,
            "event_rule.created",
            "event_rule",
            &id,
            "success",
            correlation_id,
        );
        insert_audit(&mut transaction, &entry).await?;
        transaction.commit().await.map_err(|_| AppError::Internal)?;

        sqlx::query_as::<_, EventRule>("SELECT * FROM event_rules WHERE id = ?")
            .bind(&id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| AppError::Internal)
    }

    pub async fn list_services_visible_to(
        &self,
        user_id: &str,
        is_admin: bool,
    ) -> Result<Vec<InstalledService>, AppError> {
        let query = if is_admin {
            sqlx::query_as::<_, InstalledService>(
                "SELECT * FROM installed_services ORDER BY created_at DESC, id",
            )
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as::<_, InstalledService>(
                "SELECT installed_services.* FROM installed_services \
                 JOIN operation_members ON operation_members.operation_id = installed_services.operation_id \
                 WHERE operation_members.user_id = ? \
                 ORDER BY installed_services.created_at DESC, installed_services.id",
            )
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
        };
        query.map_err(|_| AppError::Internal)
    }

    pub async fn update_operation(
        &self,
        operation_id: &str,
        name: &str,
        purpose: &str,
        scope_note: &str,
        status: OperationStatus,
        actor_id: &str,
        correlation_id: &str,
    ) -> Result<Operation, AppError> {
        let mut transaction = self.pool.begin().await.map_err(|_| AppError::Internal)?;
        sqlx::query(
            "UPDATE operations SET name = ?, purpose = ?, scope_note = ?, status = ?, \
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?",
        )
        .bind(name)
        .bind(purpose)
        .bind(scope_note)
        .bind(status)
        .bind(operation_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::Internal)?;

        let entry = AuditEntry::new(
            actor_id,
            "operation.updated",
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

    pub async fn search_portal(
        &self,
        user_id: &str,
        is_admin: bool,
        query: &str,
    ) -> Result<(Vec<Operation>, Vec<Asset>, Vec<Callback>), AppError> {
        let pattern = format!("%{}%", query);
        let operations = if is_admin {
            sqlx::query_as::<_, Operation>(
                "SELECT * FROM operations WHERE name LIKE ? OR purpose LIKE ? \
                 ORDER BY updated_at DESC, id",
            )
            .bind(&pattern)
            .bind(&pattern)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as::<_, Operation>(
                "SELECT operations.* FROM operations \
                 JOIN operation_members ON operation_members.operation_id = operations.id \
                 WHERE operation_members.user_id = ? AND (operations.name LIKE ? OR operations.purpose LIKE ?) \
                 ORDER BY operations.updated_at DESC, operations.id",
            )
            .bind(user_id)
            .bind(&pattern)
            .bind(&pattern)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|_| AppError::Internal)?;

        let assets = if is_admin {
            sqlx::query_as::<_, Asset>(
                "SELECT * FROM assets WHERE name LIKE ? OR owner LIKE ? ORDER BY updated_at DESC, id",
            )
            .bind(&pattern)
            .bind(&pattern)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as::<_, Asset>(
                "SELECT assets.* FROM assets \
                 JOIN operation_members ON operation_members.operation_id = assets.operation_id \
                 WHERE operation_members.user_id = ? AND (assets.name LIKE ? OR assets.owner LIKE ?) \
                 ORDER BY assets.updated_at DESC, assets.id",
            )
            .bind(user_id)
            .bind(&pattern)
            .bind(&pattern)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|_| AppError::Internal)?;

        let callbacks = if is_admin {
            sqlx::query_as::<_, Callback>(
                "SELECT * FROM callbacks WHERE host LIKE ? ORDER BY last_seen DESC, id",
            )
            .bind(&pattern)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as::<_, Callback>(
                "SELECT callbacks.* FROM callbacks \
                 JOIN operation_members ON operation_members.operation_id = callbacks.operation_id \
                 WHERE operation_members.user_id = ? AND callbacks.host LIKE ? \
                 ORDER BY callbacks.last_seen DESC, callbacks.id",
            )
            .bind(user_id)
            .bind(&pattern)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|_| AppError::Internal)?;

        Ok((operations, assets, callbacks))
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

fn u64_to_i64(value: u64) -> i64 {
    value.try_into().unwrap_or(i64::MAX)
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
