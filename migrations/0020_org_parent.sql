-- The interim org hierarchy: an org has at most one parent. An org that belongs to a business is what the business
-- invoices; a client's org is made with the client, with the client's own id, under the business that has the client.
ALTER TABLE organizations ADD COLUMN IF NOT EXISTS parent_id UUID REFERENCES organizations (id);
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'organizations_not_own_parent') THEN
        ALTER TABLE organizations ADD CONSTRAINT organizations_not_own_parent CHECK (parent_id <> id);
    END IF;
END $$;
CREATE INDEX IF NOT EXISTS organizations_parent ON organizations (parent_id) WHERE parent_id IS NOT NULL;

CREATE OR REPLACE FUNCTION clients_make_org() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO organizations (id, name, timezone, parent_id)
    SELECT NEW.id, NEW.name, o.timezone, NEW.org_id FROM organizations o WHERE o.id = NEW.org_id
    ON CONFLICT (id) DO NOTHING;
    RETURN NULL;
END $$;
DROP TRIGGER IF EXISTS trg_clients_org ON clients;
CREATE TRIGGER trg_clients_org AFTER INSERT ON clients FOR EACH ROW EXECUTE FUNCTION clients_make_org();

-- The clients from before the trigger.
INSERT INTO organizations (id, name, timezone, parent_id)
SELECT c.id, c.name, o.timezone, c.org_id FROM clients c JOIN organizations o ON o.id = c.org_id
WHERE NOT EXISTS (SELECT 1 FROM organizations x WHERE x.id = c.id);
