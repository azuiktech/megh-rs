-- Booking schedules (open time) and blockouts (blocked time): a recurrence and the window of each day it gives.
CREATE TABLE IF NOT EXISTS schedules (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    name TEXT NOT NULL,
    provider_id UUID,
    resource_id UUID,
    service_ids UUID[] NOT NULL DEFAULT '{}',
    recurrence JSONB NOT NULL,
    starts_at TIME NOT NULL,
    ends_at TIME NOT NULL,
    attributes JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, provider_id) REFERENCES providers (org_id, id),
    FOREIGN KEY (org_id, resource_id) REFERENCES resources (org_id, id)
);

CREATE TABLE IF NOT EXISTS blockouts (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    name TEXT NOT NULL,
    provider_id UUID,
    resource_id UUID,
    service_ids UUID[] NOT NULL DEFAULT '{}',
    recurrence JSONB NOT NULL,
    starts_at TIME NOT NULL,
    ends_at TIME NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    attributes JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, provider_id) REFERENCES providers (org_id, id),
    FOREIGN KEY (org_id, resource_id) REFERENCES resources (org_id, id)
);
