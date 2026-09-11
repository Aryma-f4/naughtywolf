CREATE TABLE IF NOT EXISTS c2_transfer_leases (
    transfer_id TEXT PRIMARY KEY REFERENCES c2_file_transfers(id) ON DELETE CASCADE,
    owner TEXT NOT NULL,
    expires_at INTEGER NOT NULL
);
