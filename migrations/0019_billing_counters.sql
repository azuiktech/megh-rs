-- The running count behind the ids of invoices and payments: one row per org, kind of document and scope (the date part
-- of the id, so a new year starts again at 1). `next` is the next number plus one, as the first one given is 1. The
-- stamps are the ones a cascading soft delete needs of every table that hangs off `organizations`.
CREATE TABLE IF NOT EXISTS billing_counters (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    document TEXT NOT NULL CHECK (document IN ('invoice', 'payment')),
    scope TEXT NOT NULL,
    next BIGINT NOT NULL CHECK (next >= 2),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, document, scope)
);
