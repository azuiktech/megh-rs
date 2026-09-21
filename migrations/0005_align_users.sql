-- Align users with megh-go (GORM AutoMigrate): account_id, provider, password_hash as nullable text;
-- unique (account_id, provider). Idempotent: databases are shared with megh-go.
ALTER TABLE users ADD COLUMN IF NOT EXISTS account_id TEXT;
ALTER TABLE users ADD COLUMN IF NOT EXISTS provider TEXT;
ALTER TABLE users ADD COLUMN IF NOT EXISTS password_hash TEXT;
ALTER TABLE users ALTER COLUMN email TYPE TEXT;

-- subject was "provider:account_id"; move it into the two columns, then drop it.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_schema = current_schema() AND table_name = 'users' AND column_name = 'subject'
    ) THEN
        UPDATE users
        SET provider = split_part(subject, ':', 1),
            account_id = substr(subject, strpos(subject, ':') + 1)
        WHERE account_id IS NULL AND provider IS NULL AND strpos(subject, ':') > 0;

        ALTER TABLE users DROP COLUMN subject;
    END IF;
END $$;

CREATE UNIQUE INDEX IF NOT EXISTS idx_users_account ON users (account_id, provider);
