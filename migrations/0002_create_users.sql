CREATE TABLE IF NOT EXISTS users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    subject VARCHAR(255) NOT NULL,
    email VARCHAR(255) NOT NULL,
    display_name VARCHAR(255) NOT NULL DEFAULT '',
    photo_url VARCHAR(1024) NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_users_subject UNIQUE (subject),
    CONSTRAINT uq_users_email UNIQUE (email)
);

CREATE INDEX IF NOT EXISTS idx_users_email ON users(email);
-- Migration 0005 drops `subject`; once dropped it is not indexed again.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_schema = current_schema() AND table_name = 'users' AND column_name = 'subject'
    ) THEN
        CREATE INDEX IF NOT EXISTS idx_users_subject ON users(subject);
    END IF;
END $$;
