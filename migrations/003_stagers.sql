-- migrations/003_stagers.sql
CREATE TABLE IF NOT EXISTS stager_templates (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name text UNIQUE NOT NULL,
    description text NOT NULL DEFAULT '',
    goos text NOT NULL DEFAULT 'linux',
    goarch text NOT NULL DEFAULT 'amd64',
    format int NOT NULL DEFAULT 2,
    protocol text NOT NULL DEFAULT 'mtls',
    is_beacon boolean NOT NULL DEFAULT false,
    obfuscate boolean NOT NULL DEFAULT true,
    sample_count int NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX idx_stager_templates_goos_arch ON stager_templates(goos, goarch);
