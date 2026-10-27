-- Products sold with services (docs/TPD-billing.md section 7). A supplier is a client with a role.
ALTER TABLE clients ADD COLUMN IF NOT EXISTS role TEXT;

-- A product belongs to one tenant, its owner; the SKU is unique per owner. Its id alone is unique too, so a stock of
-- another tenant can name it.
CREATE TABLE IF NOT EXISTS products (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id UUID NOT NULL UNIQUE,
    sku TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    unit TEXT NOT NULL,
    price BIGINT NOT NULL,
    currency TEXT NOT NULL CHECK (char_length(currency) = 3),
    attributes JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id)
);
CREATE UNIQUE INDEX IF NOT EXISTS products_sku ON products (tenant_id, sku) WHERE deleted_at IS NULL;

-- A service stored before prices costs 0 USD a session until its price is changed.
ALTER TABLE services ADD COLUMN IF NOT EXISTS unit TEXT NOT NULL DEFAULT 'session';
ALTER TABLE services ADD COLUMN IF NOT EXISTS price BIGINT NOT NULL DEFAULT 0;
ALTER TABLE services ADD COLUMN IF NOT EXISTS currency TEXT NOT NULL DEFAULT 'USD' CHECK (char_length(currency) = 3);

-- The price ledger: a row each time a product or service is created or its price or currency changes.
CREATE TABLE IF NOT EXISTS prices (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    item_id UUID NOT NULL,
    item_kind TEXT NOT NULL CHECK (item_kind IN ('product', 'service')),
    price BIGINT NOT NULL,
    currency TEXT NOT NULL CHECK (char_length(currency) = 3),
    effective_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id)
);
CREATE INDEX IF NOT EXISTS prices_item ON prices (tenant_id, item_id, effective_at) WHERE deleted_at IS NULL;

-- One row per tenant and product it holds.
CREATE TABLE IF NOT EXISTS stocks (
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    product_id UUID NOT NULL REFERENCES products (id),
    on_hand NUMERIC NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, id),
    UNIQUE (tenant_id, product_id)
);
