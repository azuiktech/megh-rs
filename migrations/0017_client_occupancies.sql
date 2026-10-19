-- The client is an axis like a bookable: a row for the client's own time, found by one indexed range scan.
ALTER TABLE occupancies ALTER COLUMN bookable_id DROP NOT NULL;
ALTER TABLE occupancies ADD COLUMN IF NOT EXISTS client_id UUID REFERENCES clients(id) ON DELETE CASCADE;
ALTER TABLE occupancies DROP CONSTRAINT IF EXISTS occupancies_one_axis;
ALTER TABLE occupancies ADD CONSTRAINT occupancies_one_axis CHECK ((bookable_id IS NULL) <> (client_id IS NULL));
CREATE INDEX IF NOT EXISTS occupancies_client ON occupancies (client_id, status, starts_at, ends_at) WHERE deleted_at IS NULL AND client_id IS NOT NULL;
