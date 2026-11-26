-- Users, the provider accounts they connect, and their sessions (SQLite; migrations/postgres/0001_auth.sql).
-- A uuid is a 16-byte BLOB, as sqlx writes it; a new id is a v7 made here. A time is RFC 3339 text, as sqlx writes it.
-- An email compares without case (COLLATE NOCASE). A NULL written to a column with a default is that default, as
-- `SET column = DEFAULT` is on PostgreSQL (NOT NULL ON CONFLICT REPLACE).
CREATE TABLE IF NOT EXISTS users (
    id BLOB PRIMARY KEY DEFAULT (unhex(printf('%012x', CAST(unixepoch('subsec') * 1000 AS INTEGER)) || '7' || substr(hex(randomblob(2)), 2, 3) || substr('89ab', 1 + abs(random()) % 4, 1) || substr(hex(randomblob(2)), 2, 3) || hex(randomblob(6)))),
    email TEXT NOT NULL COLLATE NOCASE,
    display_name TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '',
    photo_url TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '',
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    account_id TEXT,
    provider TEXT,
    password_hash TEXT,
    deleted_at TEXT,
    CONSTRAINT uq_users_email UNIQUE (email)
);
CREATE INDEX IF NOT EXISTS idx_users_email ON users (email);
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_account ON users (account_id, provider);

-- With the columns of 0007 and 0008.
CREATE TABLE IF NOT EXISTS connected_accounts (
    account_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    email TEXT COLLATE NOCASE,
    access_token TEXT NOT NULL,
    refresh_token TEXT,
    token_type TEXT DEFAULT 'Bearer',
    expiry TEXT,
    created_at TEXT DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    disconnected_at TEXT,
    deleted_at TEXT,
    id BLOB NOT NULL ON CONFLICT REPLACE DEFAULT (unhex(printf('%012x', CAST(unixepoch('subsec') * 1000 AS INTEGER)) || '7' || substr(hex(randomblob(2)), 2, 3) || substr('89ab', 1 + abs(random()) % 4, 1) || substr(hex(randomblob(2)), 2, 3) || hex(randomblob(6)))),
    tenant_id BLOB REFERENCES tenants (id) ON DELETE CASCADE,
    url TEXT,
    icon_url TEXT,
    auth TEXT CHECK (auth IN ('oauth', 'bearer', 'api_key')),
    key_header TEXT,
    oauth TEXT,
    spec_url TEXT,
    PRIMARY KEY (account_id, provider)
);
CREATE INDEX IF NOT EXISTS idx_connected_accounts_email ON connected_accounts (email);

CREATE TABLE IF NOT EXISTS sessions (
    id BLOB PRIMARY KEY DEFAULT (unhex(printf('%012x', CAST(unixepoch('subsec') * 1000 AS INTEGER)) || '7' || substr(hex(randomblob(2)), 2, 3) || substr('89ab', 1 + abs(random()) % 4, 1) || substr(hex(randomblob(2)), 2, 3) || hex(randomblob(6)))),
    user_id BLOB NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    user_agent TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '',
    ip_address TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '',
    metadata TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '{}',
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    CONSTRAINT uq_sessions_token_hash UNIQUE (token_hash)
);
CREATE INDEX IF NOT EXISTS idx_sessions_user_id ON sessions (user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_token_hash ON sessions (token_hash);
CREATE INDEX IF NOT EXISTS idx_sessions_expires_at ON sessions (expires_at);
