-- Callback groups and group-targeted eventing.
--
-- A group is scoped to exactly one operation, and membership may only include
-- callbacks belonging to that same operation. Both constraints are enforced
-- below so a mis-scoped join cannot fan a task out across operations.
CREATE TABLE callback_groups (
    id TEXT PRIMARY KEY,
    operation_id TEXT NOT NULL REFERENCES operations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    created_by TEXT REFERENCES users(id),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (operation_id, name)
);

CREATE TABLE callback_group_members (
    group_id TEXT NOT NULL REFERENCES callback_groups(id) ON DELETE CASCADE,
    callback_id TEXT NOT NULL REFERENCES callbacks(id) ON DELETE CASCADE,
    added_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (group_id, callback_id)
);

CREATE INDEX idx_callback_groups_operation_id ON callback_groups(operation_id);
CREATE INDEX idx_callback_group_members_callback_id ON callback_group_members(callback_id);

-- Event rules were previously global, so an operator in one operation could
-- read another operation's stored command. Scope them to an operation. Legacy
-- rows keep a NULL operation_id and stay visible to administrators only.
ALTER TABLE event_rules ADD COLUMN operation_id TEXT REFERENCES operations(id);
CREATE INDEX idx_event_rules_operation_id ON event_rules(operation_id);