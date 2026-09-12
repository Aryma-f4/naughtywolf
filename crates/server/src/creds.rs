use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

/// A harvested credential record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    pub id: String,
    pub source: String,
    pub cred_type: String,
    pub session_id: String,
    pub secret: String,
}

/// In-memory credential store (no persistence). Used in tests and when
/// the server runs without a database.
#[derive(Default, Clone)]
pub struct CredentialStore {
    _pool: Option<SqlitePool>,
    mem: Arc<Vec<Credential>>,
}

impl CredentialStore {
    /// Create a store backed by the given SQLite pool.
    pub fn new(pool: SqlitePool) -> Self {
        CredentialStore {
            _pool: Some(pool),
            mem: Arc::new(Vec::new()),
        }
    }

    /// Create a store with no persistence (purely in-memory).
    pub fn new_in_memory() -> Self {
        CredentialStore::default()
    }

    /// Store a credential.
    pub async fn store(&self, _cred: Credential) {
        // In-memory only; in a full impl this would INSERT into a creds table.
    }

    /// List credentials harvested for a specific session.
    pub async fn list(&self, sid: &str) -> Vec<Credential> {
        self.mem
            .iter()
            .filter(|c| c.session_id == sid)
            .cloned()
            .collect()
    }

    /// List all harvested credentials (admin only).
    pub async fn list_all(&self) -> Vec<Credential> {
        self.mem.iter().cloned().collect()
    }
}

pub type SharedCredStore = Arc<CredentialStore>;
