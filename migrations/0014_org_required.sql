-- Every entity belongs to an org. A table that still has rows without one fails here, on purpose: nobody decides for you
-- which org they belong to.
ALTER TABLE clients ALTER COLUMN org_id SET NOT NULL;
ALTER TABLE services ALTER COLUMN org_id SET NOT NULL;
ALTER TABLE payment_methods ALTER COLUMN org_id SET NOT NULL;
ALTER TABLE groupings ALTER COLUMN org_id SET NOT NULL;
ALTER TABLE schedules ALTER COLUMN org_id SET NOT NULL;
ALTER TABLE appointments ALTER COLUMN org_id SET NOT NULL;
ALTER TABLE holds ALTER COLUMN org_id SET NOT NULL;
ALTER TABLE waitlist_entries ALTER COLUMN org_id SET NOT NULL;

-- The parts of a client's name and how to reach them.
ALTER TABLE clients ADD COLUMN IF NOT EXISTS first_name TEXT NOT NULL DEFAULT '';
ALTER TABLE clients ADD COLUMN IF NOT EXISTS last_name TEXT NOT NULL DEFAULT '';
ALTER TABLE clients ADD COLUMN IF NOT EXISTS email TEXT;
ALTER TABLE clients ADD COLUMN IF NOT EXISTS home_phone TEXT;
ALTER TABLE clients ADD COLUMN IF NOT EXISTS work_phone TEXT;
ALTER TABLE clients ADD COLUMN IF NOT EXISTS mobile_phone TEXT;
ALTER TABLE clients ALTER COLUMN first_name DROP DEFAULT;
ALTER TABLE clients ALTER COLUMN last_name DROP DEFAULT;
