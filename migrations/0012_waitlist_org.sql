-- A waitlist entry belongs to the org of the REST path that made it; NULL for entries made from Rust.
ALTER TABLE waitlist_entries ADD COLUMN IF NOT EXISTS org_id UUID REFERENCES organizations(id) ON DELETE CASCADE;
