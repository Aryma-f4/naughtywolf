use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use nw_profile::msgs::{FileAck, FileChunk};
use uuid::Uuid;

/// Storekey for a transfer: (session, remote basename).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key(Uuid, String);

/// Persisted progress for one transfer.
#[derive(Debug, Clone)]
struct Job {
    received: u64,
    total: u64,
    done: bool,
}

/// On-disk store for files pulled off agents. Writes chunks by absolute
/// offset into `downloads/<session>-<basename>` and tracks the highest
/// contiguous offset so an interrupted transfer resumes on re-issue.
///
/// ponytail: synchronous fs behind one Mutex is fine for the low per-beacon
/// write volume of a C2; switch to async/batched I/O only if a session ever
/// streams many large files at once.
#[derive(Clone)]
pub struct FileStore {
    inner: Arc<Mutex<HashMap<Key, Job>>>,
    dir: Arc<PathBuf>,
}

// Default to the system temp dir so any default-backed store writes off-tree
// (a test/embedder that omits a dir never litters the working copy).
impl Default for FileStore {
    fn default() -> Self {
        FileStore::new(std::env::temp_dir())
    }
}

impl FileStore {
    pub fn new(dir: PathBuf) -> Self {
        std::fs::create_dir_all(&dir).ok();
        FileStore {
            inner: Arc::new(Mutex::new(HashMap::new())),
            dir: Arc::new(dir),
        }
    }

    /// Record one inbound chunk, appending at its absolute offset. Returns the
    /// server's revised progress for this transfer.
    pub fn write_chunk(&self, session: &Uuid, chunk: &FileChunk) -> FileAck {
        let key = Key(*session, sanitize(&chunk.name));
        let mut map = self.inner.lock().unwrap();
        let path = normalize(&self.dir, &key);

        if !map.contains_key(&key) {
            map.insert(
                key.clone(),
                Job {
                    // Resume: if a partial file already exists on disk (a prior
                    // run), seed received from its size.
                    received: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                    total: chunk.total,
                    done: false,
                },
            );
        }

        let job = map.get_mut(&key).unwrap();
        job.total = chunk.total;
        // Skip chunks we already have, so overlapping/duplicate data is safe.
        if chunk.offset >= job.received {
            let mut data = chunk.data.clone();
            // Zero-fill any gap between received and this chunk's offset so the
            // byte count stays contiguous even if a chunk was dropped.
            if chunk.offset > job.received {
                data = vec![0u8; (chunk.offset - job.received) as usize]
                    .into_iter()
                    .chain(data)
                    .collect();
            }
            if let Err(e) = append_at(&path, &data) {
                tracing::warn!(file = %path.display(), "append chunk: {e}");
                return FileAck {
                    received: job.received,
                    total: job.total,
                    done: false,
                };
            }
            job.received += data.len() as u64;
        }
        job.done = job.received >= job.total;
        FileAck {
            received: job.received,
            total: job.total,
            done: job.done,
        }
    }
}

fn append_at(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(data)
}

/// Directory-escape the remote basename: keep only the last path component and
/// strip anything that could traverse or collide.
fn sanitize(name: &str) -> String {
    name.rsplit(['/', '\\'])
        .next()
        .unwrap_or("download")
        .chars()
        .filter(|c| !c.is_control() && *c != ':' && *c != ' ')
        .take(128)
        .collect()
}

fn normalize(dir: &Path, key: &Key) -> PathBuf {
    dir.join(format!("{}-{}", key.0, key.1))
}

#[allow(dead_code)]
fn _unused() {}
