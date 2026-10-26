-- Billing: the plans a tenant subscribes to, and its invoicing (docs/TPD-billing.md section 6): taxes, invoices,
-- invoice lines, payments and the counters behind their ids, one tenant's rows keyed by (tenant_id, id).

CREATE TABLE IF NOT EXISTS plans (
    id UUID PRIMARY KEY,
    sku TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    features TEXT,
    limits TEXT,
    metadata TEXT,
    active BOOLEAN DEFAULT TRUE,
    created_at TIMESTAMPTZ,
    deleted_at TIMESTAMPTZ
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
    active BOOLEAN DEFAULT TRUE,
    deleted_at TIMESTAMPTZ
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
    active BOOLEAN DEFAULT TRUE,
    deleted_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_add_ons_sku ON add_ons (sku);

CREATE TABLE IF NOT EXISTS plan_add_ons (
    plan_id UUID NOT NULL REFERENCES plans (id),
    add_on_id UUID NOT NULL,
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (plan_id, add_on_id)
);

CREATE TABLE IF NOT EXISTS subscriptions (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL,
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
    updated_at TIMESTAMPTZ,
    deleted_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_subscriptions_external_id ON subscriptions (external_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_subscriptions_tenant_id ON subscriptions (tenant_id);

CREATE TABLE IF NOT EXISTS subscription_items (
    id UUID PRIMARY KEY,
    subscription_id UUID NOT NULL REFERENCES subscriptions (id),
    add_on_id UUID NOT NULL,
    quantity BIGINT DEFAULT 1,
    metadata TEXT,
    deleted_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_subscription_items_subscription_id ON subscription_items (subscription_id);


CREATE TABLE IF NOT EXISTS billing_taxes (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    name TEXT NOT NULL,
    rate NUMERIC NOT NULL CHECK (rate BETWEEN 0 AND 100),
    description TEXT NOT NULL DEFAULT '',
    attributes JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id)
);

-- The customer is a tenant. A line names a product (no table yet), a service and its appointment, or neither.
CREATE TABLE IF NOT EXISTS billing_invoices (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    customer_id UUID NOT NULL REFERENCES tenants (id),
    number TEXT,
    status TEXT NOT NULL CHECK (status IN ('draft', 'open', 'void')),
    date DATE NOT NULL,
    due_date DATE NOT NULL,
    currency TEXT NOT NULL CHECK (char_length(currency) = 3),
    bill_to JSONB,
    comments TEXT NOT NULL DEFAULT '',
    sub_total BIGINT NOT NULL DEFAULT 0,
    tax_total BIGINT NOT NULL DEFAULT 0,
    total BIGINT NOT NULL DEFAULT 0,
    amount_paid BIGINT NOT NULL DEFAULT 0,
    balance_due BIGINT NOT NULL DEFAULT 0,
    attributes JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id)
);
CREATE UNIQUE INDEX IF NOT EXISTS billing_invoices_number ON billing_invoices (tenant_id, number) WHERE number IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS billing_invoices_customer ON billing_invoices (tenant_id, customer_id, due_date) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS billing_invoices_owed ON billing_invoices (tenant_id, due_date) WHERE status = 'open' AND balance_due > 0 AND deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS billing_invoice_lines (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    invoice_id UUID NOT NULL,
    position INT NOT NULL,
    product_id UUID,
    service_id UUID,
    appointment_id UUID,
    description TEXT NOT NULL,
    quantity NUMERIC NOT NULL CHECK (quantity >= 0),
    unit_price BIGINT NOT NULL,
    discount NUMERIC NOT NULL DEFAULT 0 CHECK (discount BETWEEN 0 AND 100),
    tax_ids UUID[] NOT NULL DEFAULT '{}',
    sub_total BIGINT NOT NULL DEFAULT 0,
    total BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, invoice_id) REFERENCES billing_invoices (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, service_id) REFERENCES services (tenant_id, id),
    FOREIGN KEY (tenant_id, appointment_id) REFERENCES appointments (tenant_id, id),
    CHECK (num_nonnulls(product_id, service_id) <= 1)
);
CREATE INDEX IF NOT EXISTS billing_invoice_lines_invoice ON billing_invoice_lines (tenant_id, invoice_id, position) WHERE deleted_at IS NULL;

-- One payment pays one invoice; a refund is kept on the payment.
CREATE TABLE IF NOT EXISTS billing_payments (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    invoice_id UUID NOT NULL,
    number TEXT,
    date DATE NOT NULL,
    amount BIGINT NOT NULL CHECK (amount > 0),
    method TEXT NOT NULL CHECK (method IN ('cash', 'cheque', 'card', 'bank_transfer', 'other')),
    status TEXT NOT NULL CHECK (status IN ('processing', 'succeeded', 'failed', 'void')),
    refunded BIGINT NOT NULL DEFAULT 0,
    attributes JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, invoice_id) REFERENCES billing_invoices (tenant_id, id) ON DELETE CASCADE,
    CHECK (refunded >= 0 AND refunded <= amount)
);
CREATE UNIQUE INDEX IF NOT EXISTS billing_payments_number ON billing_payments (tenant_id, number) WHERE number IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS billing_payments_invoice ON billing_payments (tenant_id, invoice_id) WHERE deleted_at IS NULL;

-- The running count behind the ids of invoices and payments: one row per tenant, kind of document and scope (the date
-- part of the id, so a new year starts again at 1). `next` is the next number plus one, as the first one given is 1.
CREATE TABLE IF NOT EXISTS billing_counters (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    document TEXT NOT NULL CHECK (document IN ('invoice', 'payment')),
    scope TEXT NOT NULL,
    next BIGINT NOT NULL CHECK (next >= 2),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, document, scope)
);
