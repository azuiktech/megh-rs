-- Booking appointments: one client, one service, one room and zero or more providers for the client's time.
CREATE TABLE IF NOT EXISTS appointments (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    client_id UUID NOT NULL,
    service_id UUID NOT NULL,
    resource_id UUID NOT NULL,
    provider_ids UUID[] NOT NULL DEFAULT '{}',
    starts_at TIMESTAMPTZ NOT NULL,
    ends_at TIMESTAMPTZ NOT NULL CHECK (ends_at > starts_at),
    status TEXT NOT NULL CHECK (status IN ('waitlist', 'booked', 'confirmed', 'completed', 'cancelled')),
    note TEXT NOT NULL DEFAULT '',
    payment JSONB NOT NULL,
    booked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    attributes JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, client_id) REFERENCES clients (org_id, id),
    FOREIGN KEY (org_id, service_id) REFERENCES services (org_id, id),
    FOREIGN KEY (org_id, resource_id) REFERENCES resources (org_id, id)
);
