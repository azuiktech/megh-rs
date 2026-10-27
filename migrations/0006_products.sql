-- Products sold with services (docs/TPD-billing.md section 7). A supplier is a client with a role.
ALTER TABLE clients ADD COLUMN IF NOT EXISTS role TEXT;
