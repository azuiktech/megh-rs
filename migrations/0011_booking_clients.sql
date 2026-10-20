-- Booking clients (who books: a person, or a pet): one org's rows, keyed by (org_id, id).
CREATE TABLE IF NOT EXISTS clients (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    first_name TEXT NOT NULL,
    last_name TEXT NOT NULL,
    name TEXT NOT NULL,
    email TEXT,
    home_phone TEXT,
    work_phone TEXT,
    mobile_phone TEXT,
    gender TEXT NOT NULL DEFAULT 'unknown' CHECK (gender IN ('male', 'female', 'other', 'unknown')),
    birth_date DATE,
    address JSONB,
    attributes JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id)
);
