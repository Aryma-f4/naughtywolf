pub mod middleware;
pub mod password;
pub mod rbac;

use sqlx::FromRow;

use crate::{db::repositories::Repository, error::AppError};

use self::{password::verify_password, rbac::Role};

#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub id: String,
    pub username: String,
    pub role: Role,
}

impl AuthenticatedUser {
    pub fn require(&self, required: Role) -> Result<(), AppError> {
        self.role
            .allows(required)
            .then_some(())
            .ok_or(AppError::Forbidden)
    }
}

#[derive(FromRow)]
struct UserCredentials {
    id: String,
    username: String,
    password_hash: String,
    role: String,
}

#[derive(FromRow)]
struct UserIdentity {
    id: String,
    username: String,
    role: String,
}

/// Verifies credentials for an enabled local user.
///
/// A missing, disabled, or incorrectly authenticated account never yields a
/// session candidate.
pub async fn authenticate(
    repository: &Repository,
    username: &str,
    password: &str,
) -> Result<Option<AuthenticatedUser>, AppError> {
    let user = sqlx::query_as::<_, UserCredentials>(
        "SELECT id, username, password_hash, role FROM users WHERE username = ? AND disabled = 0",
    )
    .bind(username)
    .fetch_optional(&repository.pool)
    .await
    .map_err(|_| AppError::Internal)?;

    let Some(user) = user else {
        return Ok(None);
    };

    if !verify_password(password, &user.password_hash).map_err(|_| AppError::Internal)? {
        return Ok(None);
    }

    let role = user.role.parse::<Role>().map_err(|_| AppError::Internal)?;
    Ok(Some(AuthenticatedUser {
        id: user.id,
        username: user.username,
        role,
    }))
}

/// Loads the current identity for an enabled user.
///
/// This is used on every authenticated request so disabling an account takes
/// effect even when its session cookie was issued earlier.
pub async fn enabled_user_by_id(
    repository: &Repository,
    user_id: &str,
) -> Result<Option<AuthenticatedUser>, AppError> {
    let user = sqlx::query_as::<_, UserIdentity>(
        "SELECT id, username, role FROM users WHERE id = ? AND disabled = 0",
    )
    .bind(user_id)
    .fetch_optional(&repository.pool)
    .await
    .map_err(|_| AppError::Internal)?;

    let Some(user) = user else {
        return Ok(None);
    };

    let role = user.role.parse::<Role>().map_err(|_| AppError::Internal)?;
    Ok(Some(AuthenticatedUser {
        id: user.id,
        username: user.username,
        role,
    }))
}
