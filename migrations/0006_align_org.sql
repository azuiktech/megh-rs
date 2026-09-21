-- Align the org domain with megh-go (tables, columns and types as created by its GORM AutoMigrate).
-- Idempotent: databases are shared with megh-go.
--
-- Old megh-rs tables are renamed, not dropped and recreated: application tables hold foreign keys to
-- org(id) and those follow a rename. If megh-go's tables already exist next to the old ones, rows are
-- copied instead and the old tables stay (their dependents keep pointing at them).
DO $$
BEGIN
    IF to_regclass('org') IS NOT NULL AND to_regclass('organizations') IS NULL THEN
        ALTER TABLE org RENAME TO organizations;
        ALTER TABLE organizations
            ALTER COLUMN name TYPE TEXT,
            ALTER COLUMN name DROP NOT NULL,
            ALTER COLUMN description DROP NOT NULL,
            ALTER COLUMN timezone TYPE TEXT,
            ADD COLUMN sub_status TEXT DEFAULT 'trialing',
            ADD COLUMN trial_ends_at TIMESTAMPTZ,
            ADD COLUMN plan_id UUID,
            ADD COLUMN created_by UUID,
            ADD COLUMN updated_at TIMESTAMPTZ DEFAULT NOW();
    END IF;
    IF to_regclass('org_members') IS NOT NULL AND to_regclass('organization_members') IS NULL THEN
        ALTER TABLE org_members RENAME TO organization_members;
        ALTER TABLE organization_members
            ALTER COLUMN grants DROP DEFAULT,
            ALTER COLUMN grants DROP NOT NULL,
            ALTER COLUMN grants TYPE TEXT USING to_json(grants)::text,
            ADD COLUMN role TEXT DEFAULT 'member',
            ADD COLUMN invited_by UUID;
    END IF;
END $$;

CREATE TABLE IF NOT EXISTS organizations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT,
    description TEXT,
    timezone TEXT DEFAULT 'UTC',
    sub_status TEXT DEFAULT 'trialing',
    trial_ends_at TIMESTAMPTZ,
    plan_id UUID,
    created_by UUID,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS organization_members (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id UUID,
    user_id UUID,
    role TEXT DEFAULT 'member',
    grants TEXT,
    joined_at TIMESTAMPTZ DEFAULT NOW(),
    invited_by UUID,
    UNIQUE (organization_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_org_members_user_id ON organization_members (user_id);

CREATE TABLE IF NOT EXISTS organization_invites (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id UUID,
    email TEXT,
    token_hash TEXT,
    status TEXT DEFAULT 'pending',
    invited_by UUID,
    expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    accepted_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_organization_invites_token_hash ON organization_invites (token_hash);

CREATE TABLE IF NOT EXISTS org_configs (
    organization_id UUID PRIMARY KEY,
    auto_join_domains TEXT
);

-- Billing tables: schema only.
CREATE TABLE IF NOT EXISTS plans (
    id UUID PRIMARY KEY,
    sku TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    features TEXT,
    limits TEXT,
    metadata TEXT,
    active BOOLEAN DEFAULT TRUE,
    created_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_plans_sku ON plans (sku);

CREATE TABLE IF NOT EXISTS plan_prices (
    id UUID PRIMARY KEY,
    plan_id UUID NOT NULL REFERENCES plans (id),
    sku TEXT NOT NULL,
    "interval" TEXT NOT NULL,
    currency VARCHAR(3),
    base_price BIGINT NOT NULL,
    per_seat_price BIGINT DEFAULT 0,
    included_seats BIGINT DEFAULT 1,
    trial_duration BIGINT,
    metadata TEXT,
    active BOOLEAN DEFAULT TRUE
);
CREATE INDEX IF NOT EXISTS idx_plan_prices_plan_id ON plan_prices (plan_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_plan_prices_sku ON plan_prices (sku);

CREATE TABLE IF NOT EXISTS add_ons (
    id UUID PRIMARY KEY,
    sku TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    type TEXT NOT NULL,
    price BIGINT NOT NULL,
    currency VARCHAR(3),
    "interval" TEXT,
    unit TEXT DEFAULT 'unit',
    metadata TEXT,
    active BOOLEAN DEFAULT TRUE
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_add_ons_sku ON add_ons (sku);

CREATE TABLE IF NOT EXISTS plan_add_ons (
    plan_id UUID NOT NULL REFERENCES plans (id),
    add_on_id UUID NOT NULL,
    PRIMARY KEY (plan_id, add_on_id)
);

CREATE TABLE IF NOT EXISTS subscriptions (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL,
    plan_id UUID NOT NULL,
    price_id UUID NOT NULL,
    status TEXT NOT NULL,
    seats BIGINT DEFAULT 1,
    current_period_start TIMESTAMPTZ,
    current_period_end TIMESTAMPTZ,
    trial_ends_at TIMESTAMPTZ,
    cancel_at_period_end BOOLEAN DEFAULT FALSE,
    external_id TEXT,
    metadata TEXT,
    created_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_subscriptions_external_id ON subscriptions (external_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_subscriptions_organization_id ON subscriptions (organization_id);

CREATE TABLE IF NOT EXISTS subscription_items (
    id UUID PRIMARY KEY,
    subscription_id UUID NOT NULL REFERENCES subscriptions (id),
    add_on_id UUID NOT NULL,
    quantity BIGINT DEFAULT 1,
    metadata TEXT
);
CREATE INDEX IF NOT EXISTS idx_subscription_items_subscription_id ON subscription_items (subscription_id);

-- megh-go's tables already existed next to the old ones: copy rows, keep the old tables.
DO $$
BEGIN
    IF to_regclass('org') IS NOT NULL THEN
        INSERT INTO organizations (id, name, description, timezone, created_at, updated_at)
        SELECT id, name, description, timezone, created_at, created_at FROM org
        ON CONFLICT DO NOTHING;
    END IF;
    IF to_regclass('org_members') IS NOT NULL THEN
        INSERT INTO organization_members (id, organization_id, user_id, grants, joined_at)
        SELECT id, organization_id, user_id, to_json(grants)::text, joined_at FROM org_members
        ON CONFLICT DO NOTHING;
    END IF;
END $$;
