-- An edge of the grouping graph belongs to the org that made it (the org of the REST path); NULL for edges made from Rust.
ALTER TABLE groupings ADD COLUMN IF NOT EXISTS org_id UUID REFERENCES organizations(id) ON DELETE CASCADE;
