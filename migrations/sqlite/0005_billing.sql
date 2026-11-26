-- Billing (SQLite; migrations/postgres/0005_billing.sql), with the invoice columns 0006 adds. A decimal is text.
CREATE TABLE IF NOT EXISTS plans (
    id BLOB PRIMARY KEY,
    sku TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    features TEXT,
    limits TEXT,
    metadata TEXT,
    active BOOLEAN DEFAULT TRUE,
    created_at TEXT,
    deleted_at TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_plans_sku ON plans (sku);

CREATE TABLE IF NOT EXISTS plan_prices (
    id BLOB PRIMARY KEY,
    plan_id BLOB NOT NULL REFERENCES plans (id),
    sku TEXT NOT NULL,
    "interval" TEXT NOT NULL,
    currency TEXT,
    base_price INTEGER NOT NULL,
    per_seat_price INTEGER DEFAULT 0,
    included_seats INTEGER DEFAULT 1,
    trial_duration INTEGER,
    metadata TEXT,
    active BOOLEAN DEFAULT TRUE,
    deleted_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_plan_prices_plan_id ON plan_prices (plan_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_plan_prices_sku ON plan_prices (sku);

CREATE TABLE IF NOT EXISTS add_ons (
    id BLOB PRIMARY KEY,
    sku TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    type TEXT NOT NULL,
    price INTEGER NOT NULL,
    currency TEXT,
    "interval" TEXT,
    unit TEXT DEFAULT 'unit',
    metadata TEXT,
    active BOOLEAN DEFAULT TRUE,
    deleted_at TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_add_ons_sku ON add_ons (sku);

CREATE TABLE IF NOT EXISTS plan_add_ons (
    plan_id BLOB NOT NULL REFERENCES plans (id),
    add_on_id BLOB NOT NULL,
    deleted_at TEXT,
    PRIMARY KEY (plan_id, add_on_id)
);

CREATE TABLE IF NOT EXISTS subscriptions (
    id BLOB PRIMARY KEY,
    tenant_id BLOB NOT NULL,
    plan_id BLOB NOT NULL,
    price_id BLOB NOT NULL,
    status TEXT NOT NULL,
    seats INTEGER DEFAULT 1,
    current_period_start TEXT,
    current_period_end TEXT,
    trial_ends_at TEXT,
    cancel_at_period_end BOOLEAN DEFAULT FALSE,
    external_id TEXT,
    metadata TEXT,
    created_at TEXT,
    updated_at TEXT,
    deleted_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_subscriptions_external_id ON subscriptions (external_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_subscriptions_tenant_id ON subscriptions (tenant_id);

CREATE TABLE IF NOT EXISTS subscription_items (
    id BLOB PRIMARY KEY,
    subscription_id BLOB NOT NULL REFERENCES subscriptions (id),
    add_on_id BLOB NOT NULL,
    quantity INTEGER DEFAULT 1,
    metadata TEXT,
    deleted_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_subscription_items_subscription_id ON subscription_items (subscription_id);

CREATE TABLE IF NOT EXISTS billing_taxes (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id BLOB NOT NULL,
    name TEXT NOT NULL,
    rate TEXT NOT NULL CHECK (CAST(rate AS REAL) BETWEEN 0 AND 100),
    description TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '',
    attributes TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '{}',
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    deleted_at TEXT,
    PRIMARY KEY (tenant_id, id)
);

CREATE TABLE IF NOT EXISTS billing_invoices (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id BLOB NOT NULL,
    party_id BLOB NOT NULL,
    number TEXT,
    status TEXT NOT NULL CHECK (status IN ('draft', 'open', 'void')),
    date TEXT NOT NULL,
    due_date TEXT NOT NULL,
    currency TEXT NOT NULL CHECK (length(currency) = 3),
    bill_to TEXT,
    comments TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '',
    sub_total INTEGER NOT NULL ON CONFLICT REPLACE DEFAULT 0,
    tax_total INTEGER NOT NULL ON CONFLICT REPLACE DEFAULT 0,
    total INTEGER NOT NULL ON CONFLICT REPLACE DEFAULT 0,
    amount_paid INTEGER NOT NULL ON CONFLICT REPLACE DEFAULT 0,
    balance_due INTEGER NOT NULL ON CONFLICT REPLACE DEFAULT 0,
    attributes TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '{}',
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    deleted_at TEXT,
    party_kind TEXT NOT NULL ON CONFLICT REPLACE DEFAULT 'tenant' CHECK (party_kind IN ('client', 'tenant')),
    kind TEXT NOT NULL ON CONFLICT REPLACE DEFAULT 'receivable' CHECK (kind IN ('receivable', 'payable')),
    reference TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '',
    delivery_date TEXT,
    deliver_to TEXT,
    PRIMARY KEY (tenant_id, id)
);
CREATE UNIQUE INDEX IF NOT EXISTS billing_invoices_number ON billing_invoices (tenant_id, number) WHERE number IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS billing_invoices_owed ON billing_invoices (tenant_id, due_date) WHERE status = 'open' AND balance_due > 0 AND deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS billing_invoices_party ON billing_invoices (tenant_id, party_id, due_date) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS billing_invoice_lines (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id BLOB NOT NULL,
    invoice_id BLOB NOT NULL,
    position INTEGER NOT NULL,
    product_id BLOB,
    service_id BLOB,
    appointment_id BLOB,
    description TEXT NOT NULL,
    quantity TEXT NOT NULL CHECK (CAST(quantity AS REAL) >= 0),
    unit_price INTEGER NOT NULL,
    discount TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '0' CHECK (CAST(discount AS REAL) BETWEEN 0 AND 100),
    tax_ids TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '[]',
    sub_total INTEGER NOT NULL ON CONFLICT REPLACE DEFAULT 0,
    total INTEGER NOT NULL ON CONFLICT REPLACE DEFAULT 0,
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    deleted_at TEXT,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, invoice_id) REFERENCES billing_invoices (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, service_id) REFERENCES services (tenant_id, id),
    FOREIGN KEY (tenant_id, appointment_id) REFERENCES appointments (tenant_id, id),
    CHECK ((product_id IS NOT NULL) + (service_id IS NOT NULL) <= 1)
);
CREATE INDEX IF NOT EXISTS billing_invoice_lines_invoice ON billing_invoice_lines (tenant_id, invoice_id, position) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS billing_payments (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id BLOB NOT NULL,
    invoice_id BLOB NOT NULL,
    number TEXT,
    date TEXT NOT NULL,
    amount INTEGER NOT NULL CHECK (amount > 0),
    method TEXT NOT NULL CHECK (method IN ('cash', 'cheque', 'card', 'bank_transfer', 'other')),
    status TEXT NOT NULL CHECK (status IN ('processing', 'succeeded', 'failed', 'void')),
    refunded INTEGER NOT NULL ON CONFLICT REPLACE DEFAULT 0,
    attributes TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '{}',
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    deleted_at TEXT,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, invoice_id) REFERENCES billing_invoices (tenant_id, id) ON DELETE CASCADE,
    CHECK (refunded >= 0 AND refunded <= amount)
);
CREATE UNIQUE INDEX IF NOT EXISTS billing_payments_number ON billing_payments (tenant_id, number) WHERE number IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS billing_payments_invoice ON billing_payments (tenant_id, invoice_id) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS billing_counters (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    document TEXT NOT NULL CHECK (document IN ('invoice', 'payment')),
    scope TEXT NOT NULL,
    next INTEGER NOT NULL CHECK (next >= 2),
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    deleted_at TEXT,
    PRIMARY KEY (tenant_id, document, scope)
);
