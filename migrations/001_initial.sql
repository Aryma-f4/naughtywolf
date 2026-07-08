-- migrations/001_initial.sql
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

DO $$ BEGIN
    CREATE TYPE user_role AS ENUM ('admin', 'operator', 'viewer');
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;

CREATE TABLE IF NOT EXISTS users (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    username text UNIQUE NOT NULL,
    password_hash text NOT NULL,
    role user_role NOT NULL DEFAULT 'operator',
    disabled bool NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS sliver_profiles (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name text NOT NULL UNIQUE,
    config_path text NOT NULL,
    operator_name text NOT NULL,
    lhost text NOT NULL,
    lport integer NOT NULL,
    fingerprint text,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS user_sliver_profiles (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    profile_id uuid NOT NULL REFERENCES sliver_profiles(id) ON DELETE CASCADE,
    default_profile bool NOT NULL DEFAULT false,
    PRIMARY KEY (user_id, profile_id)
);

CREATE TABLE IF NOT EXISTS audit_events (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid REFERENCES users(id),
    profile_id uuid REFERENCES sliver_profiles(id),
    action text NOT NULL,
    target_type text NOT NULL,
    target_id text,
    parameter_summary jsonb,
    result_status text NOT NULL,
    result_ref text,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS ui_preferences (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    key text NOT NULL,
    value jsonb NOT NULL,
    PRIMARY KEY (user_id, key)
);

CREATE INDEX idx_audit_events_created_at ON audit_events(created_at DESC);
CREATE INDEX idx_audit_events_user_id ON audit_events(user_id);
CREATE INDEX idx_audit_events_action ON audit_events(action);
CREATE INDEX idx_user_sliver_profiles_user_id ON user_sliver_profiles(user_id);
