-- migrations/002_credentials.sql
CREATE TABLE IF NOT EXISTS credentials (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    cred_type text NOT NULL DEFAULT 'plaintext',
    domain text NOT NULL DEFAULT '',
    username text NOT NULL DEFAULT '',
    password text NOT NULL DEFAULT '',
    host text NOT NULL DEFAULT '',
    os text NOT NULL DEFAULT '',
    sid text NOT NULL DEFAULT '',
    notes text NOT NULL DEFAULT '',
    source text NOT NULL DEFAULT 'manual',
    agent_id text,
    is_cracked bool NOT NULL DEFAULT false,
    hash text,
    hash_type text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX idx_credentials_domain ON credentials(domain);
CREATE INDEX idx_credentials_username ON credentials(username);
CREATE INDEX idx_credentials_host ON credentials(host);
CREATE INDEX idx_credentials_source ON credentials(source);
