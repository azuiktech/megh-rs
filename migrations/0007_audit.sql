-- Row-change auditing as in megh-go: the shared audit_entries table (same columns and indexes as its GORM
-- AutoMigrate creates) and the functions that record and install it. Idempotent: databases are shared with megh-go.
CREATE TABLE IF NOT EXISTS audit_entries (
    id UUID PRIMARY KEY,
    table_name TEXT NOT NULL,
    record_id TEXT NOT NULL,
    action TEXT NOT NULL,
    actor_id TEXT,
    old_data TEXT,
    new_data TEXT,
    created_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_audit_entries_target_table ON audit_entries (table_name);
CREATE INDEX IF NOT EXISTS idx_audit_entries_record_id ON audit_entries (record_id);
CREATE INDEX IF NOT EXISTS idx_audit_entries_actor_id ON audit_entries (actor_id);
CREATE INDEX IF NOT EXISTS idx_audit_entries_created_at ON audit_entries (created_at);

-- The trigger function shared by every audited table. Arguments: the audit table and the key column.
-- The actor is the transaction-local setting megh.actor_id (empty or unset means none).
CREATE OR REPLACE FUNCTION megh_audit() RETURNS TRIGGER AS $$
DECLARE
    changed JSONB := to_jsonb(CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END);
BEGIN
    EXECUTE format(
        'INSERT INTO %I (id, table_name, record_id, action, actor_id, old_data, new_data, created_at)
         VALUES (gen_random_uuid(), $1, $2, $3, NULLIF(current_setting(''megh.actor_id'', true), ''''), $4, $5, clock_timestamp())',
        TG_ARGV[0]
    ) USING
        TG_TABLE_NAME,
        changed ->> TG_ARGV[1],
        TG_OP,
        CASE WHEN TG_OP = 'INSERT' THEN NULL ELSE to_jsonb(OLD)::text END,
        CASE WHEN TG_OP = 'DELETE' THEN NULL ELSE to_jsonb(NEW)::text END;
    RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
END;
$$ LANGUAGE plpgsql;

-- Creates the audit table (shaped like audit_entries) and the trigger, each only when missing. Every name is quoted.
CREATE OR REPLACE FUNCTION megh_audit_install(target TEXT, audit_table TEXT, trigger_name TEXT, key TEXT) RETURNS VOID AS $$
BEGIN
    IF to_regclass(format('%I', audit_table)) IS NULL THEN
        EXECUTE format('CREATE TABLE %I (LIKE audit_entries INCLUDING ALL)', audit_table);
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_trigger WHERE tgname = trigger_name AND tgrelid = to_regclass(format('%I', target))) THEN
        EXECUTE format(
            'CREATE TRIGGER %I AFTER INSERT OR UPDATE OR DELETE ON %I FOR EACH ROW EXECUTE FUNCTION megh_audit(%L, %L)',
            trigger_name, target, audit_table, key
        );
    END IF;
END;
$$ LANGUAGE plpgsql;

-- The changes recorded for one row, oldest first.
CREATE OR REPLACE FUNCTION megh_audit_history(audit_table TEXT, target TEXT, record TEXT) RETURNS SETOF audit_entries AS $$
BEGIN
    RETURN QUERY EXECUTE format('SELECT * FROM %I WHERE table_name = $1 AND record_id = $2 ORDER BY created_at', audit_table)
        USING target, record;
END;
$$ LANGUAGE plpgsql;
