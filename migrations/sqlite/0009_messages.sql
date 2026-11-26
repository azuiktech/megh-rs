-- Messaging (SQLite; migrations/postgres/0009_messages.sql).
CREATE TABLE IF NOT EXISTS messages (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id BLOB NOT NULL,
    client_id BLOB NOT NULL,
    member_id BLOB NOT NULL,
    sender TEXT NOT NULL CHECK (sender IN ('client', 'member')),
    body TEXT NOT NULL,
    read_at TEXT,
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    PRIMARY KEY (tenant_id, id)
);
CREATE INDEX IF NOT EXISTS messages_pair ON messages (tenant_id, client_id, member_id, id);
CREATE INDEX IF NOT EXISTS messages_member ON messages (tenant_id, member_id, id);
