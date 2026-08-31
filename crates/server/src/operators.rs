//! Operator model: authentication, roles, and authorization.
//!
//! Ports the portal's RBAC + argon2 password hashing into nw-server so the
//! console can require login. Operators have roles: admin, operator, viewer.

use std::fmt;
use std::str::FromStr;

use sqlx::FromRow;
use uuid::Uuid;

/// Operator role hierarchy: admin > operator > viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Role {
    Admin,
    Operator,
    Viewer,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::Admin => write!(f, "admin"),
            Role::Operator => write!(f, "operator"),
            Role::Viewer => write!(f, "viewer"),
        }
    }
}

impl FromStr for Role {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "admin" => Ok(Role::Admin),
            "operator" => Ok(Role::Operator),
            "viewer" => Ok(Role::Viewer),
            _ => Err(()),
        }
    }
}

impl Role {
    pub fn allows(self, required: Self) -> bool {
        match (self, required) {
            (Role::Admin, _) => true,
            (Role::Operator, Role::Operator | Role::Viewer) => true,
            (Role::Operator, Role::Admin) => false,
            (Role::Viewer, Role::Viewer) => true,
            (Role::Viewer, _) => false,
        }
    }
}

/// An authenticated operator session.
#[derive(Debug, Clone)]
pub struct Operator {
    pub id: Uuid,
    pub username: String,
    pub role: Role,
}

/// Row from the c2_operators table.
#[derive(FromRow)]
struct OperatorRow {
    id: String,
    username: String,
    password_hash: String,
    role: String,
}

/// In-process operator store backed by SQLite.
#[derive(Clone)]
pub struct OperatorStore {
    pool: sqlx::SqlitePool,
}

impl OperatorStore {
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        OperatorStore { pool }
    }

    /// Verify credentials and return the operator if valid + enabled.
    pub async fn authenticate(&self, username: &str, password: &str) -> Option<Operator> {
        let row: Option<OperatorRow> = sqlx::query_as(
            "SELECT id, username, password_hash, role FROM c2_operators WHERE username = ?1 AND disabled = 0"
        ).bind(username)
         .fetch_optional(&self.pool)
         .await
         .ok()?;

        let row = row?;
        if !verify_password(password, &row.password_hash) {
            return None;
        }

        let role = row.role.parse::<Role>().ok()?;
        let id = Uuid::parse_str(&row.id).ok()?;
        Some(Operator {
            id,
            username: row.username,
            role,
        })
    }

    /// Create a new operator (admin-only). Returns the new operator id.
    pub async fn create(&self, username: &str, password: &str, role: Role) -> Result<Uuid, String> {
        let hash = hash_password(password).map_err(|e| format!("argon2: {}", e))?;
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO c2_operators (id, username, password_hash, role) VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(id.to_string())
        .bind(username)
        .bind(&hash)
        .bind(role.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| format!("db: {}", e))?;
        Ok(id)
    }

    /// Seed a default admin account if no operators exist.
    pub async fn seed_default_admin(&self, default_password: &str) -> Result<(), String> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM c2_operators")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("db: {}", e))?;
        if count == 0 {
            self.create("admin", default_password, Role::Admin).await?;
        }
        Ok(())
    }

    /// Enforce the role hierarchy for an authenticated operator.
    pub fn require(&self, operator: &Operator, required: Role) -> Result<(), String> {
        if operator.role.allows(required) {
            Ok(())
        } else {
            Err(format!(
                "{} role required (current role: {})",
                required, operator.role
            ))
        }
    }

    pub async fn is_empty(&self) -> Result<bool, String> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM c2_operators")
            .fetch_one(&self.pool)
            .await
            .map_err(|error| format!("db: {error}"))?;
        Ok(count == 0)
    }
}

/// Verify a password against an argon2 hash.
fn verify_password(password: &str, hash: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    let parsed = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    argon2::Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Hash a password with argon2 + random salt.
fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    use argon2::password_hash::{PasswordHasher, SaltString, rand_core::OsRng};
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = argon2::Argon2::default();
    let hash = argon2.hash_password(password.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn seed_and_authenticate() {
        let pool = crate::persist::open_pool(":memory:").await.unwrap();
        let store = OperatorStore::new(pool);
        store.seed_default_admin("changeme123").await.unwrap();

        assert!(store.authenticate("admin", "changeme123").await.is_some());
        assert!(store.authenticate("admin", "wrong").await.is_none());
    }

    #[tokio::test]
    async fn create_operator_with_role() {
        let pool = crate::persist::open_pool(":memory:").await.unwrap();
        let store = OperatorStore::new(pool);
        let id = store
            .create("op1", "pass123", Role::Operator)
            .await
            .unwrap();

        let op = store.authenticate("op1", "pass123").await.unwrap();
        assert_eq!(op.id, id);
        assert_eq!(op.role, Role::Operator);
    }

    #[test]
    fn role_hierarchy() {
        assert!(Role::Admin.allows(Role::Admin));
        assert!(Role::Admin.allows(Role::Operator));
        assert!(Role::Admin.allows(Role::Viewer));
        assert!(!Role::Operator.allows(Role::Admin));
        assert!(Role::Operator.allows(Role::Viewer));
        assert!(!Role::Viewer.allows(Role::Operator));
        assert!(!Role::Viewer.allows(Role::Admin));
    }

    #[tokio::test]
    async fn require_enforces_role_hierarchy() {
        let pool = crate::persist::open_pool(":memory:").await.unwrap();
        let store = OperatorStore::new(pool);
        let viewer = Operator {
            id: Uuid::new_v4(),
            username: "viewer".into(),
            role: Role::Viewer,
        };
        assert!(store.require(&viewer, Role::Viewer).is_ok());
        assert!(store.require(&viewer, Role::Operator).is_err());
    }
}
