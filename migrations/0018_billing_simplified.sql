-- Billing documents (docs/TPD-billing.md section 6): taxes, invoices, invoice lines and payments, one org's rows keyed
-- by (org_id, id). This replaces the unreleased migration 0017, whose vendors, orders, order lines, payment terms, series,
-- applications and first invoices and payments are dropped when found (once, by the check on `billing_orders`).
DO $$
BEGIN
    IF to_regclass('billing_orders') IS NOT NULL THEN
        DROP TABLE IF EXISTS billing_applications, billing_payments, billing_invoice_lines, billing_invoices,
            billing_order_lines, billing_orders, billing_vendors, billing_payment_terms, billing_series CASCADE;
    END IF;
END $$;

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

-- The customer is an org. A line names a product (no table yet), a service and its appointment, or neither.
CREATE TABLE IF NOT EXISTS billing_invoices (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    customer_id UUID NOT NULL REFERENCES organizations (id),
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
    PRIMARY KEY (org_id, id)
);
CREATE UNIQUE INDEX IF NOT EXISTS billing_invoices_number ON billing_invoices (org_id, number) WHERE number IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS billing_invoices_customer ON billing_invoices (org_id, customer_id, due_date) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS billing_invoices_owed ON billing_invoices (org_id, due_date) WHERE status = 'open' AND balance_due > 0 AND deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS billing_invoice_lines (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
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
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, invoice_id) REFERENCES billing_invoices (org_id, id) ON DELETE CASCADE,
    FOREIGN KEY (org_id, service_id) REFERENCES services (org_id, id),
    FOREIGN KEY (org_id, appointment_id) REFERENCES appointments (org_id, id),
    CHECK (num_nonnulls(product_id, service_id) <= 1)
);
CREATE INDEX IF NOT EXISTS billing_invoice_lines_invoice ON billing_invoice_lines (org_id, invoice_id, position) WHERE deleted_at IS NULL;

-- One payment pays one invoice; a refund is kept on the payment.
CREATE TABLE IF NOT EXISTS billing_payments (
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
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
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, invoice_id) REFERENCES billing_invoices (org_id, id) ON DELETE CASCADE,
    CHECK (refunded >= 0 AND refunded <= amount)
);
CREATE UNIQUE INDEX IF NOT EXISTS billing_payments_number ON billing_payments (org_id, number) WHERE number IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS billing_payments_invoice ON billing_payments (org_id, invoice_id) WHERE deleted_at IS NULL;
