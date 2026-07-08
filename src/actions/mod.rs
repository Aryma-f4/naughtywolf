pub mod listeners;
pub mod sessions;
pub mod beacons;
pub mod websites;
pub mod payloads;
pub mod loot;
pub mod creds;

use crate::audit;
use crate::auth::AuthenticatedUser;
use sqlx::PgPool;
use uuid::Uuid;

pub async fn audit_action(
    pool: &PgPool,
    user: &AuthenticatedUser,
    profile_id: Option<Uuid>,
    action: &str,
    target_type: &str,
    target_id: Option<String>,
    params: Option<serde_json::Value>,
    status: &str,
) {
    audit::record_action(pool, audit::ActionRecord {
        user_id: Some(user.id),
        profile_id,
        action: action.to_string(),
        target_type: target_type.to_string(),
        target_id,
        parameter_summary: params,
        result_status: status.to_string(),
        result_ref: None,
    }).await.ok();
}
