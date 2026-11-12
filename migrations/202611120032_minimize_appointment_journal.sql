-- The migrator owns this table but is subject to FORCE RLS. DDL and data
-- conversion run in one transaction, holding the table lock until FORCE returns.
ALTER TABLE bookings.appointment_commands NO FORCE ROW LEVEL SECURITY;

DO $migration$
BEGIN
    IF EXISTS (
        SELECT 1 FROM bookings.appointment_commands
        WHERE result IS NOT NULL AND coalesce(result#>>'{resource,kind}', result->>'kind', '') <> 'appointment'
    ) THEN
        RAISE EXCEPTION 'unexpected appointment journal result';
    END IF;
END $migration$;

UPDATE bookings.appointment_commands
SET result = jsonb_build_object(
    'kind', 'appointment',
    'id', coalesce(result#>>'{resource,item,id}', result->>'id')::uuid,
    'version', coalesce(result#>>'{resource,item,version}', result->>'version')::integer
)
WHERE result IS NOT NULL;

ALTER TABLE bookings.appointment_commands ADD CONSTRAINT appointment_commands_minimal_result CHECK (
    result IS NULL OR ((
        jsonb_typeof(result) = 'object'
        AND result ?& ARRAY['kind', 'id', 'version']
        AND result - ARRAY['kind', 'id', 'version'] = '{}'::jsonb
        AND result->>'kind' = 'appointment'
        AND jsonb_typeof(result->'id') = 'string'
        AND result->>'id' ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
        AND jsonb_typeof(result->'version') = 'number'
        AND result->>'version' ~ '^[1-9][0-9]*$'
    ) IS TRUE)
);

ALTER TABLE bookings.appointment_commands FORCE ROW LEVEL SECURITY;
