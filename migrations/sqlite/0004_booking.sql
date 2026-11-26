-- Booking (SQLite; migrations/postgres/0004_booking.sql), with the columns 0006 and 0011 add. A list of uuids is the
-- text of a JSON array; a reservation's time is its start and end, where PostgreSQL keeps a range.
CREATE TABLE IF NOT EXISTS resources (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id BLOB NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '',
    role TEXT,
    capacity INTEGER NOT NULL ON CONFLICT REPLACE DEFAULT 1 CHECK (capacity >= 1),
    waitlist INTEGER NOT NULL ON CONFLICT REPLACE DEFAULT 0 CHECK (waitlist >= 0),
    attributes TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '{}',
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    deleted_at TEXT,
    PRIMARY KEY (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS providers (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id BLOB NOT NULL,
    first_name TEXT NOT NULL,
    last_name TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '',
    role TEXT,
    email TEXT,
    home_phone TEXT,
    work_phone TEXT,
    mobile_phone TEXT,
    capacity INTEGER NOT NULL ON CONFLICT REPLACE DEFAULT 1 CHECK (capacity >= 1),
    waitlist INTEGER NOT NULL ON CONFLICT REPLACE DEFAULT 0 CHECK (waitlist >= 0),
    attributes TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '{}',
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    deleted_at TEXT,
    PRIMARY KEY (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS clients (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id BLOB NOT NULL,
    first_name TEXT NOT NULL,
    last_name TEXT NOT NULL,
    name TEXT NOT NULL,
    email TEXT,
    home_phone TEXT,
    work_phone TEXT,
    mobile_phone TEXT,
    gender TEXT NOT NULL ON CONFLICT REPLACE DEFAULT 'unknown' CHECK (gender IN ('male', 'female', 'other', 'unknown')),
    birth_date TEXT,
    address TEXT,
    attributes TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '{}',
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    deleted_at TEXT,
    role TEXT,
    moved_to BLOB,
    PRIMARY KEY (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS services (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id BLOB NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '',
    setup TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '"P0D"',
    cleanup TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '"P0D"',
    steps TEXT NOT NULL,
    attributes TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '{}',
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    deleted_at TEXT,
    unit TEXT NOT NULL ON CONFLICT REPLACE DEFAULT 'session',
    price INTEGER NOT NULL ON CONFLICT REPLACE DEFAULT 0,
    currency TEXT NOT NULL ON CONFLICT REPLACE DEFAULT 'USD' CHECK (length(currency) = 3),
    PRIMARY KEY (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS schedules (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id BLOB NOT NULL,
    name TEXT NOT NULL,
    provider_id BLOB,
    resource_id BLOB,
    service_ids TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '[]',
    recurrence TEXT NOT NULL,
    starts_at TEXT NOT NULL,
    ends_at TEXT NOT NULL,
    attributes TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '{}',
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    deleted_at TEXT,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, provider_id) REFERENCES providers (tenant_id, id),
    FOREIGN KEY (tenant_id, resource_id) REFERENCES resources (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS blockouts (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id BLOB NOT NULL,
    name TEXT NOT NULL,
    provider_id BLOB,
    resource_id BLOB,
    service_ids TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '[]',
    recurrence TEXT NOT NULL,
    starts_at TEXT NOT NULL,
    ends_at TEXT NOT NULL,
    reason TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '',
    attributes TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '{}',
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    deleted_at TEXT,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, provider_id) REFERENCES providers (tenant_id, id),
    FOREIGN KEY (tenant_id, resource_id) REFERENCES resources (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS appointments (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id BLOB NOT NULL,
    client_id BLOB NOT NULL,
    service_id BLOB NOT NULL,
    resource_id BLOB NOT NULL,
    provider_ids TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '[]',
    starts_at TEXT NOT NULL,
    ends_at TEXT NOT NULL CHECK (unixepoch(ends_at, 'subsec') > unixepoch(starts_at, 'subsec')),
    status TEXT NOT NULL CHECK (status IN ('waitlist', 'booked', 'confirmed', 'completed', 'cancelled')),
    note TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '',
    payment TEXT NOT NULL,
    booked_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    attributes TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '{}',
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    deleted_at TEXT,
    products TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '[]',
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, client_id) REFERENCES clients (tenant_id, id),
    FOREIGN KEY (tenant_id, service_id) REFERENCES services (tenant_id, id),
    FOREIGN KEY (tenant_id, resource_id) REFERENCES resources (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS reservations (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id BLOB NOT NULL ON CONFLICT REPLACE DEFAULT (unhex(printf('%012x', CAST(unixepoch('subsec') * 1000 AS INTEGER)) || '7' || substr(hex(randomblob(2)), 2, 3) || substr('89ab', 1 + abs(random()) % 4, 1) || substr(hex(randomblob(2)), 2, 3) || hex(randomblob(6)))),
    owner_id BLOB NOT NULL,
    participant_id BLOB NOT NULL,
    starts_at TEXT NOT NULL,
    ends_at TEXT NOT NULL,
    waitlist BOOLEAN NOT NULL,
    expires_at TEXT,
    PRIMARY KEY (tenant_id, id)
);
CREATE INDEX IF NOT EXISTS reservations_taken ON reservations (tenant_id, participant_id, starts_at, ends_at);
CREATE INDEX IF NOT EXISTS reservations_owner ON reservations (tenant_id, owner_id);
