use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use nw_profile::msgs::{Task, TaskResult};
use uuid::Uuid;

/// Lifecycle of one task seen from the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TaskStatus {
    /// Queued, not yet delivered to the implant.
    Pending,
    /// Delivered to the implant, result not yet returned.
    Delivered,
    /// Result received from the implant.
    Completed,
}

/// Per-session task queue + delivered-result buffer.
///
/// Implant polls for tasks (drained on delivery); results are buffered until
/// the console/operator reads them (acked by the implant, then cleared). A
/// per-task status is tracked for the operator-facing `jobs` view.
#[derive(Debug)]
struct SessionQueue {
    pending: Vec<Task>,
    results: Vec<TaskResult>,
    /// task id -> (command, status); command kept so the jobs view shows what a
    /// task was even after it's been drained/cleared.
    statuses: HashMap<Uuid, (String, TaskStatus)>,
    last_task_id: u64,
}

impl SessionQueue {
    fn new() -> Self {
        SessionQueue {
            pending: Vec::new(),
            results: Vec::new(),
            statuses: HashMap::new(),
            last_task_id: 0,
        }
    }
}

#[derive(Default)]
pub struct TaskQueue {
    inner: Mutex<HashMap<Uuid, SessionQueue>>,
}

impl TaskQueue {
    pub fn new() -> Self {
        TaskQueue {
            inner: Mutex::new(HashMap::new()),
        }
    }

    fn with<F, R>(&self, session_id: &Uuid, f: F) -> R
    where
        F: FnOnce(&mut SessionQueue) -> R,
    {
        let mut map = self.inner.lock().unwrap();
        let q = map.entry(*session_id).or_insert_with(SessionQueue::new);
        f(q)
    }

    /// Enqueue a task for a session. Returns the assigned task id.
    pub fn push(
        &self,
        session_id: &Uuid,
        command: String,
        args: Vec<String>,
        timeout_ms: u64,
    ) -> Result<Uuid, QueueError> {
        let task_id = Uuid::new_v4();
        self.with(session_id, |q| {
            q.last_task_id += 1;
            q.pending.push(Task {
                id: task_id,
                command: command.clone(),
                args,
                timeout_ms,
            });
            q.statuses.insert(task_id, (command, TaskStatus::Pending));
        });
        Ok(task_id)
    }

    /// Remove and return all currently-queued tasks (drained on delivery).
    pub fn drain(&self, session_id: &Uuid) -> Vec<Task> {
        self.with(session_id, |q| {
            let taken = std::mem::take(&mut q.pending);
            for t in &taken {
                q.statuses.insert(t.id, (t.command.clone(), TaskStatus::Delivered));
            }
            taken
        })
    }

    /// Buffer a completed task result.
    pub fn deliver_result(&self, session_id: &Uuid, result: TaskResult) {
        self.with(session_id, |q| {
            q.statuses
                .entry(result.task_id)
                .and_modify(|(cmd, st)| {
                    *st = TaskStatus::Completed;
                    let _ = cmd;
                })
                .or_insert_with(|| (String::new(), TaskStatus::Completed));
            q.results.push(result);
        });
    }

    /// Current status of one task, if known.
    pub fn status(&self, session_id: &Uuid, task_id: &Uuid) -> Option<TaskStatus> {
        self.with(session_id, |q| q.statuses.get(task_id).map(|(_, st)| *st))
    }

    /// All known tasks + statuses for a session.
    pub fn statuses(&self, session_id: &Uuid) -> Vec<(String, TaskStatus)> {
        self.with(session_id, |q| {
            let mut out: Vec<(String, TaskStatus)> = q
                .statuses
                .iter()
                .map(|(_, (cmd, st))| (cmd.clone(), *st))
                .collect();
            out.sort();
            out
        })
    }

    /// Remove one delivered result by task id, if present.
    pub fn take_result(&self, session_id: &Uuid, task_id: &Uuid) -> Option<TaskResult> {
        self.with(session_id, |q| {
            let pos = q.results.iter().position(|r| r.task_id == *task_id);
            pos.map(|i| q.results.remove(i))
        })
    }

    /// All buffered-but-unread results for a session.
    pub fn results(&self, session_id: &Uuid) -> Vec<TaskResult> {
        self.with(session_id, |q| q.results.clone())
    }

    /// Remove queued tasks by id (used for kill/timeout).
    pub fn remove(&self, session_id: &Uuid, task_id: &Uuid) -> bool {
        self.with(session_id, |q| {
            let before = q.pending.len();
            q.pending.retain(|t| t.id != *task_id);
            q.pending.len() < before
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error("no such session")]
    NoSession,
}

pub type SharedQueue = Arc<TaskQueue>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_drain_take_remove() {
        let q = TaskQueue::new();
        let sid = Uuid::new_v4();
        let tid = q.push(&sid, "whoami".into(), vec![], 1000).unwrap();

        let drained = q.drain(&sid);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].id, tid);
        // second drain is empty
        assert!(q.drain(&sid).is_empty());

        q.deliver_result(
            &sid,
            TaskResult {
                task_id: tid,
                ok: true,
                stdout: b"root".to_vec(),
                stderr: vec![],
                exit_code: 0,
            },
        );
        let r = q.take_result(&sid, &tid).unwrap();
        assert_eq!(r.stdout, b"root");
        assert!(q.take_result(&sid, &tid).is_none());
    }

    #[test]
    fn remove_deletes_queued_task() {
        let q = TaskQueue::new();
        let sid = Uuid::new_v4();
        let tid = q
            .push(&sid, "sleep".into(), vec!["10".into()], 1000)
            .unwrap();
        assert!(q.remove(&sid, &tid));
        assert!(q.drain(&sid).is_empty());
    }

    #[test]
    fn status_tracks_pending_delivered_completed() {
        let q = TaskQueue::new();
        let sid = Uuid::new_v4();
        let tid = q.push(&sid, "whoami".into(), vec![], 1000).unwrap();

        assert_eq!(q.status(&sid, &tid), Some(TaskStatus::Pending));
        q.drain(&sid);
        assert_eq!(q.status(&sid, &tid), Some(TaskStatus::Delivered));

        q.deliver_result(
            &sid,
            TaskResult { task_id: tid, ok: true, stdout: vec![], stderr: vec![], exit_code: 0 },
        );
        assert_eq!(q.status(&sid, &tid), Some(TaskStatus::Completed));

        let all = q.statuses(&sid);
        assert!(all.contains(&("whoami".to_string(), TaskStatus::Completed)));
    }
}
