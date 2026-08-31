use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use nw_profile::msgs::{Task, TaskResult};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TaskStatus {
    Pending,
    Delivered,
    Completed,
}

impl TaskStatus {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(TaskStatus::Pending),
            "delivered" => Some(TaskStatus::Delivered),
            "completed" => Some(TaskStatus::Completed),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct SessionQueue {
    pending: Vec<Task>,
    results: Vec<TaskResult>,
    statuses: HashMap<Uuid, (String, TaskStatus)>,
}

impl SessionQueue {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
            results: Vec::new(),
            statuses: HashMap::new(),
        }
    }
}

enum Backend {
    Memory(Mutex<HashMap<Uuid, SessionQueue>>),
    Sqlite(sqlx::SqlitePool),
}

/// Per-session task queue backed by memory or durable SQLite.
pub struct TaskQueue {
    backend: Backend,
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(FromRow)]
struct TaskRow {
    id: String,
    command: String,
    args_json: String,
    timeout_ms: i64,
}

#[derive(FromRow)]
struct StatusRow {
    command: String,
    status: String,
}

#[derive(FromRow)]
struct ResultRow {
    task_id: String,
    ok: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: i64,
}

impl TaskQueue {
    pub fn new() -> Self {
        Self {
            backend: Backend::Memory(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_sqlite(pool: sqlx::SqlitePool) -> Self {
        Self {
            backend: Backend::Sqlite(pool),
        }
    }

    fn with_memory<F, R>(
        map: &Mutex<HashMap<Uuid, SessionQueue>>,
        session_id: &Uuid,
        operation: F,
    ) -> R
    where
        F: FnOnce(&mut SessionQueue) -> R,
    {
        let mut map = map.lock().unwrap();
        let queue = map.entry(*session_id).or_insert_with(SessionQueue::new);
        operation(queue)
    }

    pub async fn push(
        &self,
        session_id: &Uuid,
        command: String,
        args: Vec<String>,
        timeout_ms: u64,
    ) -> Result<Uuid, QueueError> {
        let task_id = Uuid::new_v4();
        match &self.backend {
            Backend::Memory(map) => Self::with_memory(map, session_id, |queue| {
                queue.pending.push(Task {
                    id: task_id,
                    command: command.clone(),
                    args,
                    timeout_ms,
                });
                queue
                    .statuses
                    .insert(task_id, (command, TaskStatus::Pending));
            }),
            Backend::Sqlite(pool) => {
                let args_json = serde_json::to_string(&args)
                    .map_err(|error| QueueError::Database(error.to_string()))?;
                let timeout = i64::try_from(timeout_ms)
                    .map_err(|error| QueueError::Database(error.to_string()))?;
                sqlx::query(
                    "INSERT INTO c2_tasks \
                     (id, session_id, command, args_json, timeout_ms, status) \
                     VALUES (?1, ?2, ?3, ?4, ?5, 'pending')",
                )
                .bind(task_id.to_string())
                .bind(session_id.to_string())
                .bind(command)
                .bind(args_json)
                .bind(timeout)
                .execute(pool)
                .await
                .map_err(|error| QueueError::Database(error.to_string()))?;
            }
        }
        Ok(task_id)
    }

    pub async fn drain(&self, session_id: &Uuid) -> Vec<Task> {
        match &self.backend {
            Backend::Memory(map) => Self::with_memory(map, session_id, |queue| {
                let tasks = std::mem::take(&mut queue.pending);
                for task in &tasks {
                    queue
                        .statuses
                        .insert(task.id, (task.command.clone(), TaskStatus::Delivered));
                }
                tasks
            }),
            Backend::Sqlite(pool) => {
                let Ok(mut transaction) = pool.begin().await else {
                    return Vec::new();
                };
                let rows = match sqlx::query_as::<_, TaskRow>(
                    "SELECT id, command, args_json, timeout_ms FROM c2_tasks \
                     WHERE session_id = ?1 AND status = 'pending' ORDER BY created_at, id",
                )
                .bind(session_id.to_string())
                .fetch_all(&mut *transaction)
                .await
                {
                    Ok(rows) => rows,
                    Err(_) => return Vec::new(),
                };
                let updated = sqlx::query(
                    "UPDATE c2_tasks SET status = 'delivered' \
                     WHERE session_id = ?1 AND status = 'pending'",
                )
                .bind(session_id.to_string())
                .execute(&mut *transaction)
                .await;
                if updated.is_err() || transaction.commit().await.is_err() {
                    return Vec::new();
                }
                rows.into_iter().filter_map(task_from_row).collect()
            }
        }
    }

    pub async fn deliver_result(&self, session_id: &Uuid, result: TaskResult) {
        match &self.backend {
            Backend::Memory(map) => Self::with_memory(map, session_id, |queue| {
                queue
                    .statuses
                    .entry(result.task_id)
                    .and_modify(|(_, status)| *status = TaskStatus::Completed)
                    .or_insert_with(|| (String::new(), TaskStatus::Completed));
                queue.results.push(result);
            }),
            Backend::Sqlite(pool) => {
                let Ok(mut transaction) = pool.begin().await else {
                    return;
                };
                let task_id = result.task_id.to_string();
                let inserted = sqlx::query(
                    "INSERT OR REPLACE INTO c2_task_results \
                     (task_id, ok, stdout, stderr, exit_code) VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .bind(&task_id)
                .bind(result.ok)
                .bind(result.stdout)
                .bind(result.stderr)
                .bind(i64::from(result.exit_code))
                .execute(&mut *transaction)
                .await;
                let updated = sqlx::query("UPDATE c2_tasks SET status = 'completed' WHERE id = ?1")
                    .bind(&task_id)
                    .execute(&mut *transaction)
                    .await;
                if inserted.is_ok() && updated.is_ok() {
                    let _ = transaction.commit().await;
                }
            }
        }
    }

    pub async fn status(&self, session_id: &Uuid, task_id: &Uuid) -> Option<TaskStatus> {
        match &self.backend {
            Backend::Memory(map) => Self::with_memory(map, session_id, |queue| {
                queue.statuses.get(task_id).map(|(_, status)| *status)
            }),
            Backend::Sqlite(pool) => {
                let status: Option<String> = sqlx::query_scalar(
                    "SELECT status FROM c2_tasks WHERE session_id = ?1 AND id = ?2",
                )
                .bind(session_id.to_string())
                .bind(task_id.to_string())
                .fetch_optional(pool)
                .await
                .ok()
                .flatten();
                status.as_deref().and_then(TaskStatus::parse)
            }
        }
    }

    pub async fn statuses(&self, session_id: &Uuid) -> Vec<(String, TaskStatus)> {
        match &self.backend {
            Backend::Memory(map) => Self::with_memory(map, session_id, |queue| {
                let mut statuses: Vec<_> = queue
                    .statuses
                    .values()
                    .map(|(command, status)| (command.clone(), *status))
                    .collect();
                statuses.sort();
                statuses
            }),
            Backend::Sqlite(pool) => sqlx::query_as::<_, StatusRow>(
                "SELECT command, status FROM c2_tasks WHERE session_id = ?1 ORDER BY created_at, id",
            )
            .bind(session_id.to_string())
            .fetch_all(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| TaskStatus::parse(&row.status).map(|status| (row.command, status)))
            .collect(),
        }
    }

    pub async fn take_result(&self, session_id: &Uuid, task_id: &Uuid) -> Option<TaskResult> {
        match &self.backend {
            Backend::Memory(map) => Self::with_memory(map, session_id, |queue| {
                let position = queue
                    .results
                    .iter()
                    .position(|result| result.task_id == *task_id)?;
                Some(queue.results.remove(position))
            }),
            Backend::Sqlite(pool) => {
                let mut transaction = pool.begin().await.ok()?;
                let row = sqlx::query_as::<_, ResultRow>(
                    "SELECT r.task_id, r.ok, r.stdout, r.stderr, r.exit_code \
                     FROM c2_task_results r JOIN c2_tasks t ON t.id = r.task_id \
                     WHERE t.session_id = ?1 AND r.task_id = ?2",
                )
                .bind(session_id.to_string())
                .bind(task_id.to_string())
                .fetch_optional(&mut *transaction)
                .await
                .ok()??;
                sqlx::query("DELETE FROM c2_task_results WHERE task_id = ?1")
                    .bind(task_id.to_string())
                    .execute(&mut *transaction)
                    .await
                    .ok()?;
                transaction.commit().await.ok()?;
                result_from_row(row)
            }
        }
    }

    pub async fn results(&self, session_id: &Uuid) -> Vec<TaskResult> {
        match &self.backend {
            Backend::Memory(map) => {
                Self::with_memory(map, session_id, |queue| queue.results.clone())
            }
            Backend::Sqlite(pool) => sqlx::query_as::<_, ResultRow>(
                "SELECT r.task_id, r.ok, r.stdout, r.stderr, r.exit_code \
                 FROM c2_task_results r JOIN c2_tasks t ON t.id = r.task_id \
                 WHERE t.session_id = ?1 ORDER BY r.completed_at, r.task_id",
            )
            .bind(session_id.to_string())
            .fetch_all(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(result_from_row)
            .collect(),
        }
    }

    pub async fn remove(&self, session_id: &Uuid, task_id: &Uuid) -> bool {
        match &self.backend {
            Backend::Memory(map) => Self::with_memory(map, session_id, |queue| {
                let before = queue.pending.len();
                queue.pending.retain(|task| task.id != *task_id);
                let removed = queue.pending.len() < before;
                if removed {
                    queue.statuses.remove(task_id);
                }
                removed
            }),
            Backend::Sqlite(pool) => sqlx::query(
                "DELETE FROM c2_tasks WHERE id = ?1 AND session_id = ?2 AND status = 'pending'",
            )
            .bind(task_id.to_string())
            .bind(session_id.to_string())
            .execute(pool)
            .await
            .map(|result| result.rows_affected() == 1)
            .unwrap_or(false),
        }
    }
}

fn task_from_row(row: TaskRow) -> Option<Task> {
    Some(Task {
        id: Uuid::parse_str(&row.id).ok()?,
        command: row.command,
        args: serde_json::from_str(&row.args_json).ok()?,
        timeout_ms: u64::try_from(row.timeout_ms).ok()?,
    })
}

fn result_from_row(row: ResultRow) -> Option<TaskResult> {
    Some(TaskResult {
        task_id: Uuid::parse_str(&row.task_id).ok()?,
        ok: row.ok,
        stdout: row.stdout,
        stderr: row.stderr,
        exit_code: i32::try_from(row.exit_code).ok()?,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error("no such session")]
    NoSession,
    #[error("database error: {0}")]
    Database(String),
}

pub type SharedQueue = Arc<TaskQueue>;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn push_drain_take_remove() {
        let queue = TaskQueue::new();
        let sid = Uuid::new_v4();
        let tid = queue
            .push(&sid, "whoami".into(), vec![], 1000)
            .await
            .unwrap();
        let drained = queue.drain(&sid).await;
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].id, tid);
        assert!(queue.drain(&sid).await.is_empty());
        queue
            .deliver_result(
                &sid,
                TaskResult {
                    task_id: tid,
                    ok: true,
                    stdout: b"root".to_vec(),
                    stderr: vec![],
                    exit_code: 0,
                },
            )
            .await;
        assert_eq!(queue.take_result(&sid, &tid).await.unwrap().stdout, b"root");
        assert!(queue.take_result(&sid, &tid).await.is_none());
    }

    #[tokio::test]
    async fn remove_deletes_queued_task() {
        let queue = TaskQueue::new();
        let sid = Uuid::new_v4();
        let tid = queue
            .push(&sid, "sleep".into(), vec!["10".into()], 1000)
            .await
            .unwrap();
        assert!(queue.remove(&sid, &tid).await);
        assert!(queue.drain(&sid).await.is_empty());
        assert_eq!(queue.status(&sid, &tid).await, None);
    }

    #[tokio::test]
    async fn status_tracks_pending_delivered_completed() {
        let queue = TaskQueue::new();
        let sid = Uuid::new_v4();
        let tid = queue
            .push(&sid, "whoami".into(), vec![], 1000)
            .await
            .unwrap();
        assert_eq!(queue.status(&sid, &tid).await, Some(TaskStatus::Pending));
        queue.drain(&sid).await;
        assert_eq!(queue.status(&sid, &tid).await, Some(TaskStatus::Delivered));
        queue
            .deliver_result(
                &sid,
                TaskResult {
                    task_id: tid,
                    ok: true,
                    stdout: vec![],
                    stderr: vec![],
                    exit_code: 0,
                },
            )
            .await;
        assert_eq!(queue.status(&sid, &tid).await, Some(TaskStatus::Completed));
        assert!(
            queue
                .statuses(&sid)
                .await
                .contains(&("whoami".to_string(), TaskStatus::Completed))
        );
    }

    #[tokio::test]
    async fn sqlite_tasks_and_results_survive_store_restart() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("c2.sqlite");
        let pool = crate::persist::open_pool(db.to_str().unwrap())
            .await
            .unwrap();
        let sid = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO c2_sessions \
             (id, hostname, username, os, arch, pid, addr, session_key, last_seen) \
             VALUES (?1, 'host', 'user', 'linux', 'x86_64', 1, '127.0.0.1', ?2, ?3)",
        )
        .bind(sid.to_string())
        .bind(vec![7u8; 32])
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();

        let first = TaskQueue::with_sqlite(pool.clone());
        let tid = first
            .push(&sid, "whoami".into(), vec![], 5_000)
            .await
            .unwrap();
        let restarted = TaskQueue::with_sqlite(pool.clone());
        assert_eq!(
            restarted.status(&sid, &tid).await,
            Some(TaskStatus::Pending)
        );
        let drained = restarted.drain(&sid).await;
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].id, tid);
        restarted
            .deliver_result(
                &sid,
                TaskResult {
                    task_id: tid,
                    ok: true,
                    stdout: b"root".to_vec(),
                    stderr: vec![],
                    exit_code: 0,
                },
            )
            .await;

        let restarted_again = TaskQueue::with_sqlite(pool);
        assert_eq!(
            restarted_again.status(&sid, &tid).await,
            Some(TaskStatus::Completed)
        );
        assert_eq!(
            restarted_again
                .take_result(&sid, &tid)
                .await
                .unwrap()
                .stdout,
            b"root"
        );
    }
}
