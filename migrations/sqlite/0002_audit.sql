-- The shared audit table (SQLite; migrations/postgres/0002_audit.sql). megh writes it from the change's transaction; the
-- PostgreSQL trigger functions have no SQLite counterpart and are not used.
CREATE TABLE IF NOT EXISTS audit_entries (
    id BLOB PRIMARY KEY,
    table_name TEXT NOT NULL,
    record_id TEXT NOT NULL,
    action TEXT NOT NULL,
    actor_id TEXT,
    old_data TEXT,
    new_data TEXT,
    created_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_audit_entries_target_table ON audit_entries (table_name);
CREATE INDEX IF NOT EXISTS idx_audit_entries_record_id ON audit_entries (record_id);
CREATE INDEX IF NOT EXISTS idx_audit_entries_actor_id ON audit_entries (actor_id);
CREATE INDEX IF NOT EXISTS idx_audit_entries_created_at ON audit_entries (created_at);
