-- Booking resources (rooms, chairs, seats, pods): one org's rows, keyed by (org_id, id).
CREATE TABLE IF NOT EXISTS resources (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    capacity INT NOT NULL DEFAULT 1 CHECK (capacity >= 1),
    waitlist INT NOT NULL DEFAULT 0 CHECK (waitlist >= 0),
    attributes JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id)
);
