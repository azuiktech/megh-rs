-- Generic booking (docs/TPD-booking.md). Every table carries the entity stamps; deleting only sets deleted_at,
-- and ON DELETE CASCADE keys are the soft-delete cascade: cancelling a booking removes its occupancies.
CREATE TABLE IF NOT EXISTS bookables (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    capacity INT NOT NULL CHECK (capacity > 0),
    attributes JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL, deleted_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS clients (
    id UUID PRIMARY KEY,
    org_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    attributes JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL, deleted_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS services (
    id UUID PRIMARY KEY,
    org_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('appointment', 'class', 'enrollment', 'resource', 'arrival')),
    price JSONB NOT NULL,
    steps JSONB NOT NULL,
    variants JSONB NOT NULL,
    attributes JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL, deleted_at TIMESTAMPTZ
);

-- One graph for every entity kind. `kind` says which table member_id and group_id point into.
CREATE TABLE IF NOT EXISTS groupings (
    id UUID PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('org', 'bookable', 'service', 'client')),
    member_id UUID NOT NULL,
    group_id UUID NOT NULL CHECK (group_id <> member_id),
    created_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL, deleted_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX IF NOT EXISTS groupings_edge ON groupings (kind, member_id, group_id) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS groupings_group ON groupings (kind, group_id) WHERE deleted_at IS NULL;

-- Schedules (open time) and blockouts (reason and notes are for blockouts). A NULL axis means any.
CREATE TABLE IF NOT EXISTS schedules (
    id UUID PRIMARY KEY,
    org_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
    service_id UUID REFERENCES services(id) ON DELETE CASCADE,
    bookable_id UUID REFERENCES bookables(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('open', 'blockout')),
    recurrence TEXT NOT NULL,
    span JSONB NOT NULL,
    reason TEXT,
    notes TEXT,
    created_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL, deleted_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS schedules_axes ON schedules (org_id, service_id, bookable_id) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS occurrences (
    id UUID PRIMARY KEY,
    org_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
    tz TEXT NOT NULL,
    service_id UUID NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    starts_at TIMESTAMPTZ NOT NULL,
    capacity INT NOT NULL CHECK (capacity > 0),
    attributes JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL, deleted_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS enrollments (
    id UUID PRIMARY KEY,
    client_id UUID NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    service_id UUID NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    occurrences JSONB NOT NULL,
    attributes JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL, deleted_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS payment_methods (
    id UUID PRIMARY KEY,
    org_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    owner_id UUID REFERENCES clients(id) ON DELETE CASCADE,
    eligible JSONB NOT NULL,
    settlement JSONB NOT NULL,
    attributes JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL, deleted_at TIMESTAMPTZ
);

-- A temporary block. Past expires_at its occupancies no longer count.
CREATE TABLE IF NOT EXISTS holds (
    id UUID PRIMARY KEY,
    org_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
    tz TEXT NOT NULL,
    parties JSONB NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    attributes JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL, deleted_at TIMESTAMPTZ
);

-- Booked, or soft-deleted when cancelled. The key and fingerprint make a confirm retry-safe.
CREATE TABLE IF NOT EXISTS appointments (
    id UUID PRIMARY KEY,
    org_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
    tz TEXT NOT NULL,
    lines JSONB NOT NULL,
    payer_id UUID NOT NULL REFERENCES clients(id),
    payments JSONB NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    fingerprint TEXT NOT NULL,
    attributes JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL, deleted_at TIMESTAMPTZ
);

-- Allocations: the only rows that block time. Owned by a hold or by a booking, never both.
CREATE TABLE IF NOT EXISTS occupancies (
    id UUID PRIMARY KEY,
    hold_id UUID REFERENCES holds(id) ON DELETE CASCADE,
    appointment_id UUID REFERENCES appointments(id) ON DELETE CASCADE,
    bookable_id UUID NOT NULL REFERENCES bookables(id) ON DELETE CASCADE,
    starts_at TIMESTAMPTZ NOT NULL,
    ends_at TIMESTAMPTZ NOT NULL CHECK (ends_at > starts_at),
    engagement TEXT NOT NULL CHECK (engagement IN ('engaged', 'free', 'on_demand')),
    seats INT NOT NULL CHECK (seats > 0),
    created_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL, deleted_at TIMESTAMPTZ,
    CHECK ((hold_id IS NULL) <> (appointment_id IS NULL))
);
CREATE INDEX IF NOT EXISTS occupancies_bookable ON occupancies (bookable_id, starts_at, ends_at) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS waitlist_entries (
    id UUID PRIMARY KEY,
    client_id UUID NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    target JSONB NOT NULL,
    requested_at TIMESTAMPTZ NOT NULL,
    attributes JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL, deleted_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS waitlist_entries_target ON waitlist_entries USING gin (target) WHERE deleted_at IS NULL;
