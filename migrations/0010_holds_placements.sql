-- A hold keeps the placements of the slot it holds, so confirming it knows each service's time and price.
ALTER TABLE holds ADD COLUMN IF NOT EXISTS placements JSONB NOT NULL DEFAULT '[]';
