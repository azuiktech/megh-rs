-- Billing documents (docs/TPD-billing.md section 6): one org's rows, keyed by (org_id, id). Lines and applications cascade
-- from their document so that a soft delete follows them. A party is a client for an invoice, credit note or sales
-- order and a vendor for the others, so party_id has no key. products have no table yet, so product_id has none.
CREATE TABLE IF NOT EXISTS billing_taxes (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    name TEXT NOT NULL,
    rate NUMERIC NOT NULL CHECK (rate BETWEEN 0 AND 100),
    description TEXT NOT NULL DEFAULT '',
    attributes JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id)
);

CREATE TABLE IF NOT EXISTS billing_payment_terms (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    name TEXT NOT NULL,
    net_days INT NOT NULL CHECK (net_days >= 0),
    description TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id)
);

CREATE TABLE IF NOT EXISTS billing_series (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    name TEXT NOT NULL,
    prefix TEXT NOT NULL DEFAULT '',
    next BIGINT NOT NULL CHECK (next >= 1),
    padding INT NOT NULL DEFAULT 0 CHECK (padding >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id)
);
CREATE UNIQUE INDEX IF NOT EXISTS billing_series_name ON billing_series (org_id, name) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS billing_vendors (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    email TEXT,
    home_phone TEXT,
    work_phone TEXT,
    mobile_phone TEXT,
    address JSONB,
    tax_id TEXT,
    payment_term_id UUID,
    currency TEXT CHECK (char_length(currency) = 3),
    attributes JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, payment_term_id) REFERENCES billing_payment_terms (org_id, id)
);

CREATE TABLE IF NOT EXISTS billing_orders (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('sales_order', 'purchase_order')),
    party_id UUID NOT NULL,
    number TEXT,
    status TEXT NOT NULL CHECK (status IN ('draft', 'open', 'cancelled')),
    date DATE NOT NULL,
    expected_date DATE,
    reference TEXT,
    currency TEXT NOT NULL CHECK (char_length(currency) = 3),
    exchange_rate NUMERIC NOT NULL DEFAULT 1 CHECK (exchange_rate > 0),
    payment_term_id UUID,
    tax_mode TEXT NOT NULL DEFAULT 'exclusive' CHECK (tax_mode IN ('exclusive', 'inclusive')),
    billing_address JSONB,
    shipping_address JSONB,
    location_id UUID REFERENCES organizations (id),
    delivery_method TEXT,
    notes TEXT NOT NULL DEFAULT '',
    terms TEXT NOT NULL DEFAULT '',
    sub_total BIGINT NOT NULL DEFAULT 0,
    tax_total BIGINT NOT NULL DEFAULT 0,
    total BIGINT NOT NULL DEFAULT 0,
    attributes JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, payment_term_id) REFERENCES billing_payment_terms (org_id, id)
);
CREATE UNIQUE INDEX IF NOT EXISTS billing_orders_number ON billing_orders (org_id, kind, number) WHERE number IS NOT NULL AND deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS billing_order_lines (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    order_id UUID NOT NULL,
    position INT NOT NULL,
    product_id UUID,
    service_id UUID,
    description TEXT NOT NULL,
    quantity NUMERIC NOT NULL CHECK (quantity >= 0),
    unit TEXT,
    unit_price BIGINT NOT NULL,
    discount NUMERIC NOT NULL DEFAULT 0 CHECK (discount BETWEEN 0 AND 100),
    tax_ids UUID[] NOT NULL DEFAULT '{}',
    expected_date DATE,
    sub_total BIGINT NOT NULL DEFAULT 0,
    total BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, order_id) REFERENCES billing_orders (org_id, id) ON DELETE CASCADE,
    FOREIGN KEY (org_id, service_id) REFERENCES services (org_id, id),
    CHECK (num_nonnulls(product_id, service_id) <= 1)
);
CREATE INDEX IF NOT EXISTS billing_order_lines_order ON billing_order_lines (org_id, order_id, position) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS billing_invoices (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('invoice', 'credit_note', 'bill', 'vendor_credit')),
    party_id UUID NOT NULL,
    number TEXT,
    status TEXT NOT NULL CHECK (status IN ('draft', 'open', 'void')),
    date DATE NOT NULL,
    due_date DATE NOT NULL,
    payment_term_id UUID,
    reference TEXT,
    currency TEXT NOT NULL CHECK (char_length(currency) = 3),
    exchange_rate NUMERIC NOT NULL DEFAULT 1 CHECK (exchange_rate > 0),
    tax_mode TEXT NOT NULL DEFAULT 'exclusive' CHECK (tax_mode IN ('exclusive', 'inclusive')),
    billing_address JSONB,
    shipping_address JSONB,
    order_id UUID,
    credited_invoice_id UUID,
    location_id UUID REFERENCES organizations (id),
    notes TEXT NOT NULL DEFAULT '',
    terms TEXT NOT NULL DEFAULT '',
    sub_total BIGINT NOT NULL DEFAULT 0,
    tax_total BIGINT NOT NULL DEFAULT 0,
    total BIGINT NOT NULL DEFAULT 0,
    attributes JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, payment_term_id) REFERENCES billing_payment_terms (org_id, id),
    FOREIGN KEY (org_id, order_id) REFERENCES billing_orders (org_id, id),
    FOREIGN KEY (org_id, credited_invoice_id) REFERENCES billing_invoices (org_id, id),
    CHECK (credited_invoice_id IS NULL OR kind IN ('credit_note', 'vendor_credit'))
);
CREATE UNIQUE INDEX IF NOT EXISTS billing_invoices_number ON billing_invoices (org_id, kind, number) WHERE number IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS billing_invoices_party ON billing_invoices (org_id, party_id, due_date) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS billing_invoice_lines (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    invoice_id UUID NOT NULL,
    position INT NOT NULL,
    product_id UUID,
    service_id UUID,
    appointment_id UUID,
    order_line_id UUID,
    description TEXT NOT NULL,
    quantity NUMERIC NOT NULL CHECK (quantity >= 0),
    unit TEXT,
    unit_price BIGINT NOT NULL,
    discount NUMERIC NOT NULL DEFAULT 0 CHECK (discount BETWEEN 0 AND 100),
    tax_ids UUID[] NOT NULL DEFAULT '{}',
    sub_total BIGINT NOT NULL DEFAULT 0,
    total BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, invoice_id) REFERENCES billing_invoices (org_id, id) ON DELETE CASCADE,
    FOREIGN KEY (org_id, service_id) REFERENCES services (org_id, id),
    FOREIGN KEY (org_id, appointment_id) REFERENCES appointments (org_id, id),
    FOREIGN KEY (org_id, order_line_id) REFERENCES billing_order_lines (org_id, id),
    CHECK (num_nonnulls(product_id, service_id) <= 1)
);
CREATE INDEX IF NOT EXISTS billing_invoice_lines_invoice ON billing_invoice_lines (org_id, invoice_id, position) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS billing_payments (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('inbound', 'outbound')),
    party_id UUID NOT NULL,
    number TEXT,
    date DATE NOT NULL,
    amount BIGINT NOT NULL CHECK (amount > 0),
    currency TEXT NOT NULL CHECK (char_length(currency) = 3),
    exchange_rate NUMERIC NOT NULL DEFAULT 1 CHECK (exchange_rate > 0),
    method TEXT NOT NULL CHECK (method IN ('cash', 'cheque', 'card', 'bank_transfer', 'other')),
    status TEXT NOT NULL CHECK (status IN ('processing', 'succeeded', 'failed', 'void')),
    reference TEXT,
    description TEXT NOT NULL DEFAULT '',
    fees BIGINT NOT NULL DEFAULT 0 CHECK (fees >= 0),
    location_id UUID REFERENCES organizations (id),
    attributes JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id)
);
CREATE UNIQUE INDEX IF NOT EXISTS billing_payments_number ON billing_payments (org_id, kind, number) WHERE number IS NOT NULL AND deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS billing_applications (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    invoice_id UUID NOT NULL,
    payment_id UUID,
    credit_id UUID,
    amount BIGINT NOT NULL CHECK (amount > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, invoice_id) REFERENCES billing_invoices (org_id, id) ON DELETE CASCADE,
    FOREIGN KEY (org_id, payment_id) REFERENCES billing_payments (org_id, id) ON DELETE CASCADE,
    FOREIGN KEY (org_id, credit_id) REFERENCES billing_invoices (org_id, id) ON DELETE CASCADE,
    CHECK (num_nonnulls(payment_id, credit_id) = 1)
);
CREATE INDEX IF NOT EXISTS billing_applications_invoice ON billing_applications (org_id, invoice_id) WHERE deleted_at IS NULL;
