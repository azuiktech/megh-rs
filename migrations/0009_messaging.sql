-- Messaging (docs/TPD-messaging.md): threads between a user and the business's agent, their participants, messages,
-- attachments and labels. One tenant's rows, keyed by (tenant_id, id). A member participant is checked when it joins,
-- not by a foreign key, so removing a member never waits on its threads.
ALTER TABLE clients ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'client';

CREATE TABLE IF NOT EXISTS threads (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    subject TEXT,
    status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'closed')),
    appointment_id UUID,
    invoice_id UUID,
    last_message_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, appointment_id) REFERENCES appointments (tenant_id, id),
    FOREIGN KEY (tenant_id, invoice_id) REFERENCES billing_invoices (tenant_id, id)
);
CREATE INDEX IF NOT EXISTS threads_recent ON threads (tenant_id, last_message_at DESC) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS participants (
    tenant_id UUID NOT NULL,
    id UUID NOT NULL,
    thread_id UUID NOT NULL,
    member_id UUID,
    client_id UUID,
    last_read_message_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, thread_id) REFERENCES threads (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, client_id) REFERENCES clients (tenant_id, id),
    CHECK ((member_id IS NULL) <> (client_id IS NULL))
);
CREATE UNIQUE INDEX IF NOT EXISTS participants_member ON participants (tenant_id, thread_id, member_id) WHERE member_id IS NOT NULL AND deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS participants_client ON participants (tenant_id, thread_id, client_id) WHERE client_id IS NOT NULL AND deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS messages (
    tenant_id UUID NOT NULL,
    id UUID NOT NULL,
    thread_id UUID NOT NULL,
    sender_id UUID NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('text', 'surface', 'answer')),
    text TEXT,
    surface JSONB,
    surface_id TEXT,
    in_reply_to UUID,
    answer JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, thread_id) REFERENCES threads (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, sender_id) REFERENCES participants (tenant_id, id),
    FOREIGN KEY (tenant_id, in_reply_to) REFERENCES messages (tenant_id, id)
);
CREATE INDEX IF NOT EXISTS messages_thread ON messages (tenant_id, thread_id, id) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS attachments (
    tenant_id UUID NOT NULL,
    id UUID NOT NULL,
    thread_id UUID NOT NULL,
    message_id UUID,
    filename TEXT NOT NULL,
    content_type TEXT NOT NULL,
    size BIGINT NOT NULL,
    content BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, thread_id) REFERENCES threads (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, message_id) REFERENCES messages (tenant_id, id)
);
CREATE INDEX IF NOT EXISTS attachments_message ON attachments (tenant_id, message_id) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS labels (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    name TEXT NOT NULL,
    colour TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS thread_labels (
    tenant_id UUID NOT NULL,
    thread_id UUID NOT NULL,
    label_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, thread_id, label_id),
    FOREIGN KEY (tenant_id, thread_id) REFERENCES threads (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, label_id) REFERENCES labels (tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS message_labels (
    tenant_id UUID NOT NULL,
    message_id UUID NOT NULL,
    label_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, message_id, label_id),
    FOREIGN KEY (tenant_id, message_id) REFERENCES messages (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, label_id) REFERENCES labels (tenant_id, id) ON DELETE CASCADE
);
