-- Booking: resources (rooms, chairs, seats, pods), providers (the people an appointment is with), clients (who books:
-- a person, or a pet), services, schedules (open time), blockouts (blocked time), appointments and the places they
-- take. One tenant's rows, keyed by (tenant_id, id).
CREATE EXTENSION IF NOT EXISTS btree_gist;

CREATE TABLE IF NOT EXISTS resources (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    role TEXT,
    capacity INT NOT NULL DEFAULT 1 CHECK (capacity >= 1),
    waitlist INT NOT NULL DEFAULT 0 CHECK (waitlist >= 0),
    attributes JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS providers (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    first_name TEXT NOT NULL,
    last_name TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    role TEXT,
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
    PRIMARY KEY (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS clients (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
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
    PRIMARY KEY (tenant_id, id)
);

-- Setup and cleanup take only the room; the steps take the client and their provider groups.
CREATE TABLE IF NOT EXISTS services (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
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
    PRIMARY KEY (tenant_id, id)
);

-- A schedule or a blockout is a recurrence and the window of each day it gives.
CREATE TABLE IF NOT EXISTS schedules (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
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
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, provider_id) REFERENCES providers (tenant_id, id),
    FOREIGN KEY (tenant_id, resource_id) REFERENCES resources (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS blockouts (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
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
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, provider_id) REFERENCES providers (tenant_id, id),
    FOREIGN KEY (tenant_id, resource_id) REFERENCES resources (tenant_id, id)
);

-- One client, one service, one room and zero or more providers for the client's time.
CREATE TABLE IF NOT EXISTS appointments (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
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
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, client_id) REFERENCES clients (tenant_id, id),
    FOREIGN KEY (tenant_id, service_id) REFERENCES services (tenant_id, id),
    FOREIGN KEY (tenant_id, resource_id) REFERENCES resources (tenant_id, id)
);

-- Places taken, one row per place: a room, provider or client during a time, by an appointment or a hold
-- (owner). Capacity left is a participant's capacity minus the most rows overlapping at once; `waitlist` rows
-- count against its waitlist instead. A hold's rows expire; cancel and release delete an owner's rows.
CREATE TABLE IF NOT EXISTS reservations (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    owner_id UUID NOT NULL,
    participant_id UUID NOT NULL,
    during TSTZRANGE NOT NULL,
    waitlist BOOLEAN NOT NULL,
    expires_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id)
);
CREATE INDEX IF NOT EXISTS reservations_taken ON reservations USING gist (tenant_id, participant_id, during);
CREATE INDEX IF NOT EXISTS reservations_owner ON reservations (tenant_id, owner_id);
