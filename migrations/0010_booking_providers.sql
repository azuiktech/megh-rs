-- Booking providers (the people an appointment is with): one org's rows, keyed by (org_id, id).
CREATE TABLE IF NOT EXISTS providers (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    first_name TEXT NOT NULL,
    last_name TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    email TEXT,
    home_phone TEXT,
    work_phone TEXT,
    mobile_phone TEXT,
    capacity INT NOT NULL DEFAULT 1 CHECK (capacity >= 1),
    waitlist INT NOT NULL DEFAULT 0 CHECK (waitlist >= 0),
    attributes JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id)
);
