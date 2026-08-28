use crate::{
    auth::{AuthenticatedUser, rbac::Role},
    db::repositories::Repository,
    error::AppError,
};

/// Authorizes a user to act within an operation at the requested role level.
///
/// Administrators satisfy the role requirement and can access every operation;
/// all other roles must be explicit operation members.
pub async fn authorize_operation(
    repo: &Repository,
    user: &AuthenticatedUser,
    operation_id: &str,
    required: Role,
) -> Result<(), AppError> {
    repo.find_operation(operation_id)
        .await?
        .ok_or(AppError::NotFound)?;
    user.require(required)?;

    if user.role == Role::Admin || repo.is_operation_member(operation_id, &user.id).await? {
        return Ok(());
    }

    Err(AppError::Forbidden)
}

/// Authorizes creation of a check run against an active asset in an active
/// operation. Membership is bypassed only for administrators.
pub async fn authorize_asset_run(
    repo: &Repository,
    user: &AuthenticatedUser,
    asset_id: &str,
) -> Result<(), AppError> {
    let asset = repo.find_asset(asset_id).await?.ok_or(AppError::NotFound)?;
    authorize_operation(repo, user, &asset.operation_id, Role::Operator).await?;
    repo.require_active_operation(&asset.operation_id).await?;
    repo.require_active_asset(asset_id).await
}
