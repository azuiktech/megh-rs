-- An appointment is one client, one service, a room and zero or more providers, from `starts_at` to `ends_at`. The rows
-- of `occupancies` are the index of the time it blocks: written when it is booked, from its service, and read by range scans.
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = 'appointments' AND column_name = 'client_id') THEN
        DROP TABLE IF EXISTS occupancies;
        DROP TABLE IF EXISTS appointments CASCADE;
    END IF;
END $$;

CREATE TABLE IF NOT EXISTS appointments (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    client_id UUID NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    service_id UUID NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    provider_ids UUID[] NOT NULL,
    room_id UUID NOT NULL REFERENCES bookables(id) ON DELETE CASCADE,
    starts_at TIMESTAMPTZ NOT NULL,
    ends_at TIMESTAMPTZ NOT NULL CHECK (ends_at > starts_at),
    status TEXT NOT NULL CHECK (status IN ('held', 'waitlist', 'booked', 'confirmed', 'cancelled')),
    expires_at TIMESTAMPTZ,
    payment JSONB,
    attributes JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL, deleted_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS appointments_org ON appointments (org_id, status, starts_at) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS appointments_room ON appointments (room_id, starts_at) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS appointments_client ON appointments (client_id, starts_at) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS appointments_providers ON appointments USING gin (provider_ids) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS appointments_expiry ON appointments (expires_at) WHERE status = 'held' AND deleted_at IS NULL;

-- The time an appointment blocks on one axis: a room or provider, or the client.
CREATE TABLE IF NOT EXISTS occupancies (
    id UUID PRIMARY KEY,
    appointment_id UUID NOT NULL REFERENCES appointments(id) ON DELETE CASCADE,
    bookable_id UUID REFERENCES bookables(id) ON DELETE CASCADE,
    client_id UUID REFERENCES clients(id) ON DELETE CASCADE,
    starts_at TIMESTAMPTZ NOT NULL,
    ends_at TIMESTAMPTZ NOT NULL CHECK (ends_at > starts_at),
    engagement TEXT NOT NULL CHECK (engagement IN ('engaged', 'free', 'on_demand')),
    status TEXT NOT NULL CHECK (status IN ('held', 'waitlist', 'booked', 'confirmed', 'cancelled')),
    created_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL, deleted_at TIMESTAMPTZ,
    CHECK ((bookable_id IS NULL) <> (client_id IS NULL))
);
CREATE INDEX IF NOT EXISTS occupancies_bookable ON occupancies (bookable_id, status, starts_at, ends_at) WHERE deleted_at IS NULL AND bookable_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS occupancies_client ON occupancies (client_id, status, starts_at, ends_at) WHERE deleted_at IS NULL AND client_id IS NOT NULL;

-- The request_id of a booking (AIP-155): the same id with the same request returns the same appointments.
CREATE TABLE IF NOT EXISTS booking_requests (
    request_id TEXT PRIMARY KEY,
    fingerprint TEXT NOT NULL,
    appointment_ids UUID[] NOT NULL
);
