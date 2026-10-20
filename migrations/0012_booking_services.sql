-- Booking services: setup and cleanup take only the room; the steps take the client and their provider groups.
CREATE TABLE IF NOT EXISTS services (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    setup JSONB NOT NULL DEFAULT '"P0D"',
    cleanup JSONB NOT NULL DEFAULT '"P0D"',
    steps JSONB NOT NULL,
    attributes JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id)
);
