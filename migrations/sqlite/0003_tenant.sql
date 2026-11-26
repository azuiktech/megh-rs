-- Tenants (SQLite; migrations/postgres/0003_tenant.sql).
CREATE TABLE IF NOT EXISTS tenants (
    id BLOB PRIMARY KEY DEFAULT (unhex(printf('%012x', CAST(unixepoch('subsec') * 1000 AS INTEGER)) || '7' || substr(hex(randomblob(2)), 2, 3) || substr('89ab', 1 + abs(random()) % 4, 1) || substr(hex(randomblob(2)), 2, 3) || hex(randomblob(6)))),
    name TEXT,
    description TEXT DEFAULT '',
    email TEXT,
    home_phone TEXT,
    work_phone TEXT,
    mobile_phone TEXT,
    address TEXT,
    timezone TEXT NOT NULL ON CONFLICT REPLACE DEFAULT 'UTC',
    sub_status TEXT DEFAULT 'trialing',
    trial_ends_at TEXT,
    plan_id BLOB,
    created_by BLOB,
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00'))
);

CREATE TABLE IF NOT EXISTS tenant_members (
    id BLOB PRIMARY KEY DEFAULT (unhex(printf('%012x', CAST(unixepoch('subsec') * 1000 AS INTEGER)) || '7' || substr(hex(randomblob(2)), 2, 3) || substr('89ab', 1 + abs(random()) % 4, 1) || substr(hex(randomblob(2)), 2, 3) || hex(randomblob(6)))),
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    user_id BLOB NOT NULL,
    role TEXT DEFAULT 'member',
    grants TEXT,
    joined_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    invited_by BLOB,
    UNIQUE (tenant_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_tenant_members_user_id ON tenant_members (user_id);
CREATE INDEX IF NOT EXISTS idx_tenant_members_tenant_id ON tenant_members (tenant_id);

CREATE TABLE IF NOT EXISTS tenant_invites (
    id BLOB PRIMARY KEY DEFAULT (unhex(printf('%012x', CAST(unixepoch('subsec') * 1000 AS INTEGER)) || '7' || substr(hex(randomblob(2)), 2, 3) || substr('89ab', 1 + abs(random()) % 4, 1) || substr(hex(randomblob(2)), 2, 3) || hex(randomblob(6)))),
    tenant_id BLOB,
    email TEXT,
    token_hash TEXT,
    status TEXT DEFAULT 'pending',
    invited_by BLOB,
    expires_at TEXT,
    created_at TEXT DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    accepted_at TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_tenant_invites_token_hash ON tenant_invites (token_hash);

CREATE TABLE IF NOT EXISTS tenant_configs (
    tenant_id BLOB PRIMARY KEY,
    auto_join_domains TEXT
);
