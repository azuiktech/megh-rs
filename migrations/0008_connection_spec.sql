-- A connection's OpenAPI document, which describes the REST API at its url.
ALTER TABLE connected_accounts ADD COLUMN IF NOT EXISTS spec_url TEXT;
