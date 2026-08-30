use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use nw_profile::crypto;
use uuid::Uuid;

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

/// In-process session registry. Persistence to sqlite lands in M2; M1 keeps
/// the working slice simple. // ponytail: in-memory, add sqlite when sessions must survive restart
#[derive(Default)]
pub struct SessionRegistry {
    inner: Mutex<HashMap<Uuid, Session>>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        SessionRegistry {
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn create(
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
        let mut map = self.inner.lock().unwrap();
        map.insert(
            id,
            Session::new(id, hostname, username, os, arch, pid, addr, key),
        );
        id
    }

    pub fn get(&self, id: &Uuid) -> Option<Session> {
        self.inner.lock().unwrap().get(id).cloned()
    }

    pub fn list(&self) -> Vec<Session> {
        let mut v: Vec<Session> = self.inner.lock().unwrap().values().cloned().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    pub fn touch(&self, id: &Uuid) -> bool {
        let mut map = self.inner.lock().unwrap();
        match map.get_mut(id) {
            Some(s) => {
                s.last_seen = std::time::SystemTime::now();
                true
            }
            None => false,
        }
    }

    pub fn remove(&self, id: &Uuid) -> bool {
        self.inner.lock().unwrap().remove(id).is_some()
    }
}

/// Shared handle for handlers.
pub type SharedRegistry = Arc<SessionRegistry>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_get_list_remove() {
        let reg = SessionRegistry::new();
        let id = reg.create(
            "h".into(),
            "u".into(),
            "linux".into(),
            "x86_64".into(),
            1,
            "1.2.3.4".into(),
            [9u8; 32],
        );
        assert_eq!(reg.list().len(), 1);
        assert_eq!(reg.get(&id).unwrap().hostname, "h");
        assert!(reg.remove(&id));
        assert_eq!(reg.list().len(), 0);
    }
}
