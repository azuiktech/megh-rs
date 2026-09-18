CREATE TABLE IF NOT EXISTS connected_accounts (
    account_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    email TEXT,
    access_token TEXT NOT NULL,
    refresh_token TEXT,
    token_type TEXT DEFAULT 'Bearer',
    expiry TIMESTAMPTZ,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW(),
    disconnected_at TIMESTAMPTZ,
    PRIMARY KEY (account_id, provider)
);

CREATE INDEX IF NOT EXISTS idx_connected_accounts_email ON connected_accounts(email);
