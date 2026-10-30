-- A tenant's connections: known providers connected by OAuth, and generic sites by URL with a bearer token, an API key
-- or OAuth. A generic connection's account_id is its own id, so tenants connecting the same URL keep their own.
ALTER TABLE connected_accounts ADD COLUMN IF NOT EXISTS id UUID NOT NULL DEFAULT uuidv7();
ALTER TABLE connected_accounts ADD COLUMN IF NOT EXISTS tenant_id UUID REFERENCES tenants (id) ON DELETE CASCADE;
ALTER TABLE connected_accounts ADD COLUMN IF NOT EXISTS url TEXT;
ALTER TABLE connected_accounts ADD COLUMN IF NOT EXISTS icon_url TEXT;
ALTER TABLE connected_accounts ADD COLUMN IF NOT EXISTS auth TEXT CHECK (auth IN ('oauth', 'bearer', 'api_key'));
ALTER TABLE connected_accounts ADD COLUMN IF NOT EXISTS key_header TEXT;
-- A site connected by OAuth: the client megh registered as and its tokens, encrypted like the tokens.
ALTER TABLE connected_accounts ADD COLUMN IF NOT EXISTS oauth TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_connected_accounts_id ON connected_accounts (id);
CREATE INDEX IF NOT EXISTS idx_connected_accounts_tenant ON connected_accounts (tenant_id) WHERE deleted_at IS NULL;
