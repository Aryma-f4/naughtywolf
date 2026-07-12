use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

/// User record (full DB row) for the admin user-management endpoints.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct UserAdmin {
    pub id: Uuid,
    pub username: String,
    pub role: String,
    pub disabled: bool,
    pub created_at: String,
}

/// Single server setting (key-value).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct SettingEntry {
    pub key: String,
    pub value: serde_json::Value,
    pub description: String,
    pub updated_at: String,
}

/// Health snapshot returned by the health endpoint.
#[derive(Debug, Serialize)]
pub struct HealthReport {
    pub status: String,
    pub db_ok: bool,
    pub sliver_connected: bool,
    pub active_sessions: u32,
    pub active_listeners: u32,
    pub uptime_seconds: i64,
}

/// Meta info returned by the meta endpoint.
#[derive(Debug, Serialize)]
pub struct MetaInfo {
    pub version: String,
    pub build: String,
    pub goos: String,
    pub arch: String,
    pub started_at: String,
}

/// List all users (admin only).
pub async fn list_users(pool: &PgPool) -> Result<Vec<UserAdmin>, String> {
    let rows = sqlx::query_as::<_, UserAdmin>(
        "SELECT id, username, role::text AS role, disabled, created_at::text
         FROM users ORDER BY created_at",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Failed to list users: {e}"))?;
    Ok(rows)
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    pub role: Option<String>,
    pub disabled: Option<bool>,
}

/// Update a user role or disabled status (admin only).
pub async fn update_user(
    pool: &PgPool,
    id: &str,
    req: UpdateUserRequest,
) -> Result<UserAdmin, String> {
    let user_id = Uuid::parse_str(id).map_err(|e| format!("Invalid user id: {e}"))?;
    let new_role = req.role;
    let new_disabled = req.disabled;
    let row = sqlx::query_as::<_, UserAdmin>(
        "UPDATE users SET
            role = COALESCE($2::user_role, role),
            disabled = COALESCE($3, disabled),
            updated_at = now()
         WHERE id = $1
         RETURNING id, username, role::text AS role, disabled, created_at::text",
    )
    .bind(user_id)
    .bind(new_role)
    .bind(new_disabled)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("Failed to update user: {e}"))?;
    Ok(row)
}

/// Delete a user (admin only).
pub async fn delete_user(pool: &PgPool, id: &str) -> Result<(), String> {
    let user_id = Uuid::parse_str(id).map_err(|e| format!("Invalid user id: {e}"))?;
    let res = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(|e| format!("Failed to delete user: {e}"))?;
    if res.rows_affected() == 0 {
        return Err(format!("User '{id}' not found"));
    }
    Ok(())
}

/// List all server settings.
pub async fn list_settings(pool: &PgPool) -> Result<Vec<SettingEntry>, String> {
    let rows = sqlx::query_as::<_, SettingEntry>(
        "SELECT key, value, description, updated_at::text FROM server_settings ORDER BY key",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Failed to list settings: {e}"))?;
    Ok(rows)
}

#[derive(Debug, Deserialize)]
pub struct UpsertSettingRequest {
    pub value: serde_json::Value,
    pub description: Option<String>,
}

/// Create or update a setting (admin only).
pub async fn upsert_setting(
    pool: &PgPool,
    key: &str,
    req: UpsertSettingRequest,
    user_id: Uuid,
) -> Result<SettingEntry, String> {
    let row = sqlx::query_as::<_, SettingEntry>(
        "INSERT INTO server_settings (key, value, description, updated_at, updated_by)
         VALUES ($1, $2, $3, now(), $4)
         ON CONFLICT (key) DO UPDATE SET
            value = EXCLUDED.value,
            description = EXCLUDED.description,
            updated_at = now(),
            updated_by = EXCLUDED.updated_by
         RETURNING key, value, description, updated_at::text",
    )
    .bind(key)
    .bind(&req.value)
    .bind(req.description.unwrap_or_default())
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("Failed to upsert setting: {e}"))?;
    Ok(row)
}

/// Check that the DB is reachable and return minimal health info.
pub async fn db_ok(pool: &PgPool) -> bool {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(pool)
        .await
        .is_ok()
}

/// Build a MetaInfo snapshot from the env (call once at startup and cache).
pub fn build_meta() -> MetaInfo {
    MetaInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        build: std::env::var("NAUGHTYWOLF_BUILD").unwrap_or_else(|_| "dev".to_string()),
        goos: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        started_at: chrono::Utc::now().to_rfc3339(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_meta_info() {
        let m = build_meta();
        assert!(!m.version.is_empty());
        assert!(!m.goos.is_empty());
    }
}
