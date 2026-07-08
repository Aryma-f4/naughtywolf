pub mod middleware;
pub mod password;
pub mod rbac;

use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub id: Uuid,
    pub username: String,
    pub role: rbac::Role,
}
