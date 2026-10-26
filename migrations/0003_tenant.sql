-- Tenants: the partition every other row belongs to, with its members, invitations and auto-join domains.
CREATE TABLE IF NOT EXISTS tenants (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT,
    description TEXT DEFAULT '',
    email TEXT,
    home_phone TEXT,
    work_phone TEXT,
    mobile_phone TEXT,
    address JSONB,
    timezone TEXT NOT NULL DEFAULT 'UTC',
    sub_status TEXT DEFAULT 'trialing',
    trial_ends_at TIMESTAMPTZ,
    plan_id UUID,
    created_by UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS tenant_members (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    user_id UUID NOT NULL,
    role TEXT DEFAULT 'member',
    grants TEXT,
    joined_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    invited_by UUID,
    UNIQUE (tenant_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_tenant_members_user_id ON tenant_members (user_id);
CREATE INDEX IF NOT EXISTS idx_tenant_members_tenant_id ON tenant_members (tenant_id);

CREATE TABLE IF NOT EXISTS tenant_invites (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID,
    email TEXT,
    token_hash TEXT,
    status TEXT DEFAULT 'pending',
    invited_by UUID,
    expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    accepted_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_tenant_invites_token_hash ON tenant_invites (token_hash);

CREATE TABLE IF NOT EXISTS tenant_configs (
    tenant_id UUID PRIMARY KEY,
    auto_join_domains TEXT
);
