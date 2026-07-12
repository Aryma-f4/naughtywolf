-- migrations/004_settings.sql
CREATE TABLE IF NOT EXISTS server_settings (
    key text PRIMARY KEY,
    value jsonb NOT NULL,
    description text NOT NULL DEFAULT '',
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by uuid REFERENCES users(id)
);
