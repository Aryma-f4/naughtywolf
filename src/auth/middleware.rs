use axum::{
    extract::FromRequestParts,
    http::StatusCode,
    http::request::Parts,
    response::{IntoResponse, Response},
};
use tower_sessions::Session;

use super::AuthenticatedUser;
use super::rbac::Role;
use crate::error::AppError;

const SESSION_USER_ID_KEY: &str = "user_id";
const SESSION_USERNAME_KEY: &str = "username";
const SESSION_ROLE_KEY: &str = "role";

pub struct AuthSession {
    pub session: Session,
}

impl AuthSession {
    pub async fn login(&self, user: &AuthenticatedUser) -> Result<(), AppError> {
        self.session
            .insert(SESSION_USER_ID_KEY, user.id.clone())
            .await
            .map_err(|_| AppError::Internal)?;
        self.session
            .insert(SESSION_USERNAME_KEY, user.username.clone())
            .await
            .map_err(|_| AppError::Internal)?;
        self.session
            .insert(SESSION_ROLE_KEY, user.role.to_string())
            .await
            .map_err(|_| AppError::Internal)?;
        Ok(())
    }

    pub async fn logout(&self) {
        self.session.delete().await.ok();
    }

    pub async fn authenticated_user(&self) -> Option<AuthenticatedUser> {
        let id: String = self.session.get(SESSION_USER_ID_KEY).await.ok()??;
        let username: String = self.session.get(SESSION_USERNAME_KEY).await.ok()??;
        let role_str: Option<String> = self.session.get(SESSION_ROLE_KEY).await.ok()?;
        let role = role_str.as_deref()?.parse::<Role>().ok()?;
        Some(AuthenticatedUser { id, username, role })
    }
}

/// Axum extractor that requires authentication.
/// If not logged in, returns 401.
pub struct AuthenticatedUserGuard(pub AuthenticatedUser);

impl<S> FromRequestParts<S> for AuthenticatedUserGuard
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(parts, state)
            .await
            .map_err(|e| e.into_response())?;
        let auth = AuthSession { session };
        match auth.authenticated_user().await {
            Some(user) => Ok(AuthenticatedUserGuard(user)),
            None => Err((StatusCode::UNAUTHORIZED, "Not authenticated").into_response()),
        }
    }
}
