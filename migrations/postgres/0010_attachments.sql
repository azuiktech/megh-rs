-- Attachments (docs/TPD-messaging.md): files a client and a member of the tenant send each other, read back by the
-- pair's client in the browser and by the member's code.
CREATE TABLE IF NOT EXISTS attachments (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    client_id UUID NOT NULL,
    member_id UUID NOT NULL,
    name TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    bytes BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, id)
);
