-- The tenant connection columns (SQLite; migrations/postgres/0007_connections.sql) are in connected_accounts (0001).
CREATE UNIQUE INDEX IF NOT EXISTS idx_connected_accounts_id ON connected_accounts (id);
CREATE INDEX IF NOT EXISTS idx_connected_accounts_tenant ON connected_accounts (tenant_id) WHERE deleted_at IS NULL;
