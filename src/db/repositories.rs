use serde_json::Value;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{
    db::models::{Asset, AssetStatus, CheckRun, CheckRunState, Operation, OperationStatus},
    error::AppError,
};

/// SQL-only access to persisted lab-platform records.
#[derive(Clone)]
pub struct Repository {
    pub pool: SqlitePool,
}

impl Repository {
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

    pub async fn list_assets(&self, operation_id: &str) -> Result<Vec<Asset>, AppError> {
        sqlx::query_as::<_, Asset>("SELECT * FROM assets WHERE operation_id = ? ORDER BY name, id")
            .bind(operation_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| AppError::Internal)
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
        .bind(CheckRunState::Queued)
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

    pub async fn finish_run(
        &self,
        run_id: &str,
        state: CheckRunState,
        result_json: Option<&Value>,
        error_summary: Option<&str>,
        output_truncated: bool,
    ) -> Result<CheckRun, AppError> {
        if !matches!(
            state,
            CheckRunState::Succeeded | CheckRunState::Failed | CheckRunState::Cancelled
        ) {
            return Err(AppError::Validation(
                "a check run must finish in a terminal state".to_owned(),
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
             WHERE id = ?",
        )
        .bind(state)
        .bind(result_json)
        .bind(error_summary)
        .bind(output_truncated)
        .bind(run_id)
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
