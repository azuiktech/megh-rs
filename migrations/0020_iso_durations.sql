-- Durations are ISO 8601 text (`PT1H`) where they were {"secs", "nanos"}: service steps and variants, schedule spans.
-- A change and a settlement are tagged {"type", "value"} where they were tagged by their variant name.
CREATE OR REPLACE FUNCTION megh_iso(duration jsonb) RETURNS jsonb LANGUAGE sql IMMUTABLE AS $$
    SELECT CASE
        WHEN jsonb_typeof(duration) = 'object' AND duration ? 'secs' THEN to_jsonb(
            'PT' || (duration->>'secs')
            || CASE WHEN (duration->>'nanos')::int = 0 THEN '' ELSE '.' || regexp_replace(lpad(duration->>'nanos', 9, '0'), '0+$', '') END
            || 'S')
        ELSE duration
    END
$$;

CREATE OR REPLACE FUNCTION megh_tag(variant jsonb) RETURNS jsonb LANGUAGE sql IMMUTABLE AS $$
    SELECT CASE
        WHEN jsonb_typeof(variant) = 'object' AND NOT variant ? 'type' THEN (SELECT jsonb_build_object('type', key, 'value', value) FROM jsonb_each(variant) LIMIT 1)
        ELSE variant
    END
$$;

UPDATE services SET
    steps = COALESCE((
        SELECT jsonb_agg(CASE WHEN step ? 'duration' THEN jsonb_set(step, '{duration}', megh_iso(step->'duration')) ELSE step END ORDER BY position)
        FROM jsonb_array_elements(steps) WITH ORDINALITY AS each_step(step, position)), '[]'::jsonb),
    variants = COALESCE((
        SELECT jsonb_agg(
            jsonb_set(jsonb_set(variant, '{duration}', COALESCE(megh_tag(CASE WHEN jsonb_typeof(variant->'duration') = 'object'
                THEN (SELECT jsonb_object_agg(change.key, megh_iso(change.value)) FROM jsonb_each(variant->'duration') AS change)
                ELSE variant->'duration' END), 'null'::jsonb)),
                '{price_micros}', COALESCE(megh_tag(variant->'price_micros'), 'null'::jsonb))
            ORDER BY position)
        FROM jsonb_array_elements(variants) WITH ORDINALITY AS each_variant(variant, position)), '[]'::jsonb);

UPDATE schedules SET span = megh_iso(span);

UPDATE payment_methods SET settlement = CASE
    WHEN jsonb_typeof(settlement) = 'string' THEN jsonb_build_object('type', settlement #>> '{}')
    ELSE megh_tag(settlement)
END;

DROP FUNCTION megh_tag(jsonb);
DROP FUNCTION megh_iso(jsonb);
