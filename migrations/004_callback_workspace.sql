-- Durable callback workspace state. All additions are nullable or have defaults so
-- databases created by migrations 001-003 remain readable without backfilling.
ALTER TABLE callbacks ADD COLUMN os_version TEXT;
ALTER TABLE callbacks ADD COLUMN executable_path TEXT;
ALTER TABLE callbacks ADD COLUMN local_addr TEXT;
ALTER TABLE callbacks ADD COLUMN implant_version TEXT;
ALTER TABLE callbacks ADD COLUMN interval_ms INTEGER;
ALTER TABLE callbacks ADD COLUMN jitter_ms INTEGER;
ALTER TABLE callbacks ADD COLUMN capabilities_json TEXT;

CREATE TABLE c2_task_results_backup AS SELECT * FROM c2_task_results;
DROP TABLE c2_task_results;

ALTER TABLE c2_tasks RENAME TO c2_tasks_old;
CREATE TABLE c2_tasks_v2 (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES c2_sessions(id) ON DELETE CASCADE,
    command TEXT NOT NULL,
    args_json TEXT NOT NULL,
    timeout_ms INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'delivering', 'delivered', 'processing', 'completed', 'error', 'cancelled')) DEFAULT 'pending',
    processing_at TEXT,
    completed_at TEXT,
    result_output TEXT,
    result_ok INTEGER,
    result_exit_code INTEGER,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    operator_id TEXT,
    parent_task_id TEXT REFERENCES c2_tasks_v2(id) ON DELETE SET NULL,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    cancellation_requested_at TEXT
);
INSERT INTO c2_tasks_v2 (id, session_id, command, args_json, timeout_ms, status,
    processing_at, completed_at, result_output, result_ok, result_exit_code, created_at)
SELECT id, session_id, command, args_json, timeout_ms, status,
    processing_at, completed_at, result_output, result_ok, result_exit_code, created_at
FROM c2_tasks_old;
DROP TABLE c2_tasks_old;
ALTER TABLE c2_tasks_v2 RENAME TO c2_tasks;

CREATE TABLE c2_task_results (
    task_id TEXT PRIMARY KEY REFERENCES c2_tasks(id) ON DELETE CASCADE,
    ok INTEGER NOT NULL,
    stdout BLOB NOT NULL,
    stderr BLOB NOT NULL,
    exit_code INTEGER NOT NULL,
    completed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
INSERT INTO c2_task_results SELECT * FROM c2_task_results_backup;
DROP TABLE c2_task_results_backup;

CREATE INDEX idx_c2_tasks_session ON c2_tasks(session_id);
CREATE INDEX idx_c2_tasks_status ON c2_tasks(status);

CREATE TABLE c2_process_snapshots (
    session_id TEXT PRIMARY KEY REFERENCES c2_sessions(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES c2_tasks(id) ON DELETE CASCADE,
    schema_version TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    captured_at TEXT NOT NULL
);

CREATE TABLE c2_file_snapshots (
    session_id TEXT NOT NULL REFERENCES c2_sessions(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES c2_tasks(id) ON DELETE CASCADE,
    schema_version TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    captured_at TEXT NOT NULL,
    PRIMARY KEY(session_id, path)
);

CREATE TABLE c2_file_transfers (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES c2_sessions(id) ON DELETE CASCADE,
    task_id TEXT REFERENCES c2_tasks(id) ON DELETE SET NULL,
    direction TEXT NOT NULL CHECK(direction IN ('upload','download')),
    remote_path TEXT NOT NULL,
    storage_key TEXT NOT NULL UNIQUE,
    expected_size INTEGER,
    received_bytes INTEGER NOT NULL DEFAULT 0,
    sha256 TEXT,
    status TEXT NOT NULL CHECK(status IN ('queued','active','completed','error','cancelled')),
    error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT
);
