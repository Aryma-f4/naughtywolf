pub mod agents;
pub mod beacons;
pub mod creds;
pub mod hosts;
pub mod modules;
pub mod listeners;
pub mod loot;
pub mod payloads;
pub mod sessions;
pub mod sliver;
pub mod websites;

use crate::audit;
use crate::auth::AuthenticatedUser;
use sqlx::PgPool;
use uuid::Uuid;

#[allow(clippy::too_many_arguments)]
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
    audit::record_action(
        pool,
        audit::ActionRecord {
            user_id: Some(user.id),
            profile_id,
            action: action.to_string(),
            target_type: target_type.to_string(),
            target_id,
            parameter_summary: params,
            result_status: status.to_string(),
            result_ref: None,
        },
    )
    .await
    .ok();
}
