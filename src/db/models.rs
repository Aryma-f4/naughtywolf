use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    pub password_hash: String,
    pub role: String,
    pub disabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct SliverProfile {
    pub id: Uuid,
    pub name: String,
    pub config_path: String,
    pub operator_name: String,
    pub lhost: String,
    pub lport: i32,
    pub fingerprint: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct UserSliverProfile {
    pub user_id: Uuid,
    pub profile_id: Uuid,
    pub default_profile: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AuditEvent {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub profile_id: Option<Uuid>,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub parameter_summary: Option<serde_json::Value>,
    pub result_status: String,
    pub result_ref: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct UiPreference {
    pub user_id: Uuid,
    pub key: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Credential {
    pub id: Uuid,
    pub cred_type: String,
    pub domain: String,
    pub username: String,
    pub password: String,
    pub host: String,
    pub os: String,
    pub sid: String,
    pub notes: String,
    pub source: String,
    pub agent_id: Option<String>,
    pub is_cracked: bool,
    pub hash: Option<String>,
    pub hash_type: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct StagerTemplate {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub goos: String,
    pub goarch: String,
    pub format: i32,
    pub protocol: String,
    pub is_beacon: bool,
    pub obfuscate: bool,
    pub sample_count: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
