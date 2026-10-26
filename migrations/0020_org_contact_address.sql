-- Contact and address of an organization, laid out like booking's clients (not in megh-go).
ALTER TABLE organizations
    ADD COLUMN IF NOT EXISTS email TEXT,
    ADD COLUMN IF NOT EXISTS home_phone TEXT,
    ADD COLUMN IF NOT EXISTS work_phone TEXT,
    ADD COLUMN IF NOT EXISTS mobile_phone TEXT,
    ADD COLUMN IF NOT EXISTS address JSONB;
