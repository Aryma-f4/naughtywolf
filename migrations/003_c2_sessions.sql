-- Session + task storage for nw-server C2.
-- Sessions mirror the in-memory Session struct; tasks track status for the
-- operator `jobs` view.

CREATE TABLE IF NOT EXISTS c2_sessions (
    id TEXT PRIMARY KEY,
    hostname TEXT NOT NULL,
    username TEXT NOT NULL,
    os TEXT NOT NULL,
    arch TEXT NOT NULL,
    pid INTEGER NOT NULL,
    addr TEXT NOT NULL,
    session_key BLOB NOT NULL,
    last_seen TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS c2_tasks (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES c2_sessions(id) ON DELETE CASCADE,
    command TEXT NOT NULL,
    args_json TEXT NOT NULL,
    timeout_ms INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'delivering', 'delivered', 'processing', 'completed', 'error')) DEFAULT 'pending',
    processing_at TEXT,
    completed_at TEXT,
    result_output TEXT,
    result_ok INTEGER,
    result_exit_code INTEGER,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS c2_task_results (
    task_id TEXT PRIMARY KEY REFERENCES c2_tasks(id) ON DELETE CASCADE,
    ok INTEGER NOT NULL,
    stdout BLOB NOT NULL,
    stderr BLOB NOT NULL,
    exit_code INTEGER NOT NULL,
    completed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_c2_tasks_session ON c2_tasks(session_id);
CREATE INDEX IF NOT EXISTS idx_c2_tasks_status ON c2_tasks(status);

CREATE TABLE IF NOT EXISTS c2_operators (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('admin', 'operator', 'viewer')),
    disabled INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS c2_audit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    operator_id TEXT,
    operator_name TEXT NOT NULL,
    action TEXT NOT NULL,
    target_session TEXT,
    details TEXT NOT NULL,
    succeeded INTEGER NOT NULL,
    timestamp TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
