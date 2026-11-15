-- Messaging (docs/TPD-messaging.md): what a client and a member of the tenant say to each other, stored before it is
-- published on the broker so a reconnecting browser catches up from here.
CREATE TABLE IF NOT EXISTS messages (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    client_id UUID NOT NULL,
    member_id UUID NOT NULL,
    sender TEXT NOT NULL CHECK (sender IN ('client', 'member')),
    body JSONB NOT NULL,
    read_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, id)
);
CREATE INDEX IF NOT EXISTS messages_pair ON messages (tenant_id, client_id, member_id, id);
CREATE INDEX IF NOT EXISTS messages_member ON messages (tenant_id, member_id, id);
