-- A hold is an appointment with status 'held' and an expires_at. Holds are temporary, so the ones in flight are dropped.
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = 'occupancies' AND column_name = 'hold_id') THEN
        DELETE FROM occupancies WHERE hold_id IS NOT NULL;
    END IF;
END $$;
DROP TABLE IF EXISTS holds CASCADE;
ALTER TABLE occupancies DROP COLUMN IF EXISTS hold_id;
ALTER TABLE occupancies ALTER COLUMN appointment_id SET NOT NULL;
ALTER TABLE appointments DROP CONSTRAINT IF EXISTS appointments_status_check;
ALTER TABLE appointments ADD CONSTRAINT appointments_status_check CHECK (status IN ('held', 'waitlist', 'booked', 'confirmed', 'cancelled'));
ALTER TABLE occupancies DROP CONSTRAINT IF EXISTS occupancies_status_check;
ALTER TABLE occupancies ADD CONSTRAINT occupancies_status_check CHECK (status IN ('held', 'waitlist', 'booked', 'confirmed', 'cancelled'));
ALTER TABLE appointments ADD COLUMN IF NOT EXISTS expires_at TIMESTAMPTZ;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = 'appointments' AND column_name = 'payer_id') THEN
        ALTER TABLE appointments ALTER COLUMN payer_id DROP NOT NULL;
        ALTER TABLE appointments ALTER COLUMN idempotency_key DROP NOT NULL;
        ALTER TABLE appointments ALTER COLUMN fingerprint DROP NOT NULL;
    END IF;
END $$;
CREATE INDEX IF NOT EXISTS appointments_expiry ON appointments (expires_at) WHERE status = 'held' AND deleted_at IS NULL;
