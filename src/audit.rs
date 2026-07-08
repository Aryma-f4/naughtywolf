use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::models::AuditEvent;

#[derive(Debug, Clone)]
pub struct ActionRecord {
    pub user_id: Option<Uuid>,
    pub profile_id: Option<Uuid>,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub parameter_summary: Option<Value>,
    pub result_status: String,
    pub result_ref: Option<String>,
}

pub async fn record_action(pool: &PgPool, record: ActionRecord) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO audit_events (id, user_id, profile_id, action, target_type, target_id, parameter_summary, result_status, result_ref) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
    )
    .bind(Uuid::new_v4())
    .bind(record.user_id)
    .bind(record.profile_id)
    .bind(&record.action)
    .bind(&record.target_type)
    .bind(&record.target_id)
    .bind(&record.parameter_summary)
    .bind(&record.result_status)
    .bind(&record.result_ref)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn query_events(
    pool: &PgPool,
    limit: i64,
    offset: i64,
) -> Result<Vec<AuditEvent>, sqlx::Error> {
    sqlx::query_as::<_, AuditEvent>(
        "SELECT * FROM audit_events ORDER BY created_at DESC LIMIT $1 OFFSET $2"
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[ignore]
    #[tokio::test]
    async fn test_record_and_query_audit() {
        let database_url = std::env::var("DATABASE_URL").unwrap();
        let pool = PgPool::connect(&database_url).await.unwrap();

        let record = ActionRecord {
            user_id: None,
            profile_id: None,
            action: "test_action".to_string(),
            target_type: "test".to_string(),
            target_id: Some("test-123".to_string()),
            parameter_summary: None,
            result_status: "success".to_string(),
            result_ref: None,
        };

        record_action(&pool, record).await.unwrap();
        let events = query_events(&pool, 10, 0).await.unwrap();
        assert!(!events.is_empty());
    }
}
