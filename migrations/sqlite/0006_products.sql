-- Products, prices and stock (SQLite; migrations/postgres/0006_products.sql). The columns it adds to other tables are
-- in their CREATE TABLE.
CREATE TABLE IF NOT EXISTS products (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id BLOB NOT NULL UNIQUE,
    sku TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '',
    unit TEXT NOT NULL,
    price INTEGER NOT NULL,
    currency TEXT NOT NULL CHECK (length(currency) = 3),
    attributes TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '{}',
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    deleted_at TEXT,
    PRIMARY KEY (tenant_id, id)
);
CREATE UNIQUE INDEX IF NOT EXISTS products_sku ON products (tenant_id, sku) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS prices (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id BLOB NOT NULL,
    item_id BLOB NOT NULL,
    item_kind TEXT NOT NULL CHECK (item_kind IN ('product', 'service')),
    price INTEGER NOT NULL,
    currency TEXT NOT NULL CHECK (length(currency) = 3),
    effective_at TEXT NOT NULL,
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    deleted_at TEXT,
    PRIMARY KEY (tenant_id, id)
);
CREATE INDEX IF NOT EXISTS prices_item ON prices (tenant_id, item_id, effective_at) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS stocks (
    tenant_id BLOB NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    id BLOB NOT NULL ON CONFLICT REPLACE DEFAULT (unhex(printf('%012x', CAST(unixepoch('subsec') * 1000 AS INTEGER)) || '7' || substr(hex(randomblob(2)), 2, 3) || substr('89ab', 1 + abs(random()) % 4, 1) || substr(hex(randomblob(2)), 2, 3) || hex(randomblob(6)))),
    product_id BLOB NOT NULL REFERENCES products (id),
    on_hand TEXT NOT NULL ON CONFLICT REPLACE DEFAULT '0',
    created_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    updated_at TEXT NOT NULL ON CONFLICT REPLACE DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')),
    deleted_at TEXT,
    PRIMARY KEY (tenant_id, id),
    UNIQUE (tenant_id, product_id)
);
