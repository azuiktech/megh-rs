-- Attachments (SQLite; migrations/postgres/0010_attachments.sql).
CREATE TABLE IF NOT EXISTS attachments (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id BLOB NOT NULL,
    client_id BLOB NOT NULL,
    member_id BLOB NOT NULL,
    name TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    bytes BLOB NOT NULL,
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    PRIMARY KEY (tenant_id, id)
);
