use axum::{
    extract::FromRequestParts,
    http::StatusCode,
    http::request::Parts,
    response::{IntoResponse, Response},
};
use tower_sessions::Session;
use uuid::Uuid;

use super::AuthenticatedUser;
use super::rbac::Role;

const SESSION_USER_ID_KEY: &str = "user_id";
const SESSION_USERNAME_KEY: &str = "username";
const SESSION_ROLE_KEY: &str = "role";

pub struct AuthSession {
    pub session: Session,
}

impl AuthSession {
    pub async fn login(&self, user_id: Uuid, username: &str, role: &Role) {
        if self
            .session
            .insert(SESSION_USER_ID_KEY, user_id)
            .await
            .is_err()
        {
            tracing::warn!("Failed to insert user_id into session for user '{username}'");
        }
        if self
            .session
            .insert(SESSION_USERNAME_KEY, username.to_string())
            .await
            .is_err()
        {
            tracing::warn!("Failed to insert username into session for user '{username}'");
        }
        if self
            .session
            .insert(SESSION_ROLE_KEY, role.to_string())
            .await
            .is_err()
        {
            tracing::warn!("Failed to insert role into session for user '{username}'");
        }
    }

    pub async fn logout(&self) {
        self.session.delete().await.ok();
    }

    pub async fn authenticated_user(&self) -> Option<AuthenticatedUser> {
        let id: Uuid = self.session.get(SESSION_USER_ID_KEY).await.ok()??;
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
