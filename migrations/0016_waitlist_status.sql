-- A room has a waitlist next to its capacity; an appointment has a status, copied to its occupancies so that the
-- seats of a room are counted by one indexed range scan.
ALTER TABLE bookables ADD COLUMN IF NOT EXISTS waitlist INT NOT NULL DEFAULT 0 CHECK (waitlist >= 0);
ALTER TABLE appointments ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'booked' CHECK (status IN ('waitlist', 'booked', 'confirmed', 'cancelled'));
ALTER TABLE occupancies ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'booked' CHECK (status IN ('waitlist', 'booked', 'confirmed', 'cancelled'));
CREATE INDEX IF NOT EXISTS occupancies_status ON occupancies (bookable_id, status, starts_at, ends_at) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS appointments_status ON appointments (org_id, status) WHERE deleted_at IS NULL;
