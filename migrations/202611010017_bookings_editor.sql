CREATE SCHEMA bookings;
REVOKE ALL ON SCHEMA bookings FROM PUBLIC;
GRANT USAGE ON SCHEMA bookings TO turnesta_runtime;

CREATE TABLE bookings.specialists (
    tenant_id uuid NOT NULL,
    location_id uuid NOT NULL,
    id uuid NOT NULL,
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 120 AND name=btrim(name) AND name !~ '[[:cntrl:]]'),
    description text NOT NULL DEFAULT '' CHECK (char_length(description)<=500 AND description=btrim(description) AND description !~ '[[:cntrl:]]'),
    enabled boolean NOT NULL,
    version integer NOT NULL DEFAULT 1 CHECK (version>0),
    schedule_version bigint NOT NULL DEFAULT 1 CHECK (schedule_version>0),
    PRIMARY KEY (tenant_id,id),
    UNIQUE (tenant_id,location_id,id),
    FOREIGN KEY (tenant_id,location_id) REFERENCES core.locations (tenant_id,id)
);
CREATE TABLE bookings.presets (
    tenant_id uuid NOT NULL,
    location_id uuid NOT NULL,
    id uuid NOT NULL,
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 120 AND name=btrim(name) AND name !~ '[[:cntrl:]]'),
    duration_minutes integer CHECK (duration_minutes BETWEEN 1 AND 1440),
    enabled boolean NOT NULL,
    version integer NOT NULL DEFAULT 1 CHECK (version>0),
    PRIMARY KEY (tenant_id,id),
    FOREIGN KEY (tenant_id,location_id) REFERENCES core.locations (tenant_id,id)
);
CREATE TABLE bookings.offers (
    tenant_id uuid NOT NULL,
    location_id uuid NOT NULL,
    specialist_id uuid NOT NULL,
    id uuid NOT NULL,
    title text NOT NULL CHECK (char_length(title)<=120 AND title=btrim(title) AND title !~ '[[:cntrl:]]'),
    mode text NOT NULL CHECK (mode IN ('interval','start_only','by_agreement')),
    local_date date,
    starts_local timestamp,
    ends_local timestamp,
    starts_at timestamptz,
    ends_at timestamptz,
    time_zone text NOT NULL REFERENCES core.time_zones(name),
    places integer NOT NULL CHECK (places BETWEEN 1 AND 1000),
    state text NOT NULL CHECK (state IN ('draft','ready','withdrawn')),
    version integer NOT NULL DEFAULT 1 CHECK (version>0),
    PRIMARY KEY (tenant_id,id),
    FOREIGN KEY (tenant_id,location_id,specialist_id) REFERENCES bookings.specialists (tenant_id,location_id,id),
    CHECK (
        (mode='interval' AND local_date IS NOT NULL AND starts_local IS NOT NULL AND ends_local IS NOT NULL
            AND starts_at IS NOT NULL AND ends_at IS NOT NULL AND ends_at>starts_at AND places=1)
        OR (mode='start_only' AND local_date IS NOT NULL AND starts_local IS NOT NULL AND ends_local IS NULL
            AND starts_at IS NOT NULL AND ends_at IS NULL AND places=1)
        OR (mode='by_agreement' AND starts_local IS NULL AND ends_local IS NULL AND starts_at IS NULL AND ends_at IS NULL)
    ),
    CHECK (starts_local IS NULL OR local_date=starts_local::date)
);
CREATE INDEX specialists_location_idx ON bookings.specialists(tenant_id,location_id,id);
CREATE INDEX presets_location_idx ON bookings.presets(tenant_id,location_id,id);
CREATE INDEX offers_day_idx ON bookings.offers(tenant_id,location_id,specialist_id,local_date,id);
CREATE INDEX offers_time_idx ON bookings.offers(tenant_id,specialist_id,starts_at) WHERE state<>'withdrawn' AND starts_at IS NOT NULL;

-- Command claim and receipt live in the same transaction; an interrupted claim
-- rolls back. Readers cannot see the temporary NULL receipt before commit.
CREATE TABLE bookings.commands (
    tenant_id uuid NOT NULL,
    location_id uuid NOT NULL,
    actor_id uuid NOT NULL,
    idempotency_key uuid NOT NULL,
    command jsonb NOT NULL CHECK (jsonb_typeof(command)='object'),
    result jsonb CHECK (result IS NULL OR jsonb_typeof(result)='object'),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id,location_id,actor_id,idempotency_key),
    FOREIGN KEY (tenant_id,location_id) REFERENCES core.locations(tenant_id,id),
    FOREIGN KEY (tenant_id,actor_id) REFERENCES core.memberships(tenant_id,user_id)
);

DO $policies$
DECLARE target text;
BEGIN
    FOREACH target IN ARRAY ARRAY['specialists','presets','offers','commands'] LOOP
        EXECUTE format('ALTER TABLE bookings.%I ENABLE ROW LEVEL SECURITY',target);
        EXECUTE format('ALTER TABLE bookings.%I FORCE ROW LEVEL SECURITY',target);
        EXECUTE format($sql$CREATE POLICY tenant_read ON bookings.%I FOR SELECT TO turnesta_runtime USING (
            tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
            AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id=%1$I.tenant_id
                AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid))$sql$,target);
        EXECUTE format($sql$CREATE POLICY manager_insert ON bookings.%I FOR INSERT TO turnesta_runtime WITH CHECK (
            tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
            AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id=%1$I.tenant_id
                AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.role IN ('owner','manager')))$sql$,target);
        EXECUTE format($sql$CREATE POLICY manager_update ON bookings.%I FOR UPDATE TO turnesta_runtime USING (
            tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
            AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id=%1$I.tenant_id
                AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.role IN ('owner','manager')))
            WITH CHECK (tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
            AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id=%1$I.tenant_id
                AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.role IN ('owner','manager')))$sql$,target);
    END LOOP;
END
$policies$;
CREATE POLICY own_command ON bookings.commands AS RESTRICTIVE TO turnesta_runtime
    USING (actor_id=nullif(current_setting('app.user_id',true),'')::uuid)
    WITH CHECK (actor_id=nullif(current_setting('app.user_id',true),'')::uuid);
REVOKE ALL ON ALL TABLES IN SCHEMA bookings FROM PUBLIC;
GRANT SELECT,INSERT ON bookings.specialists,bookings.presets,bookings.offers,bookings.commands TO turnesta_runtime;
GRANT UPDATE(name,description,enabled,version,schedule_version) ON bookings.specialists TO turnesta_runtime;
GRANT UPDATE(name,duration_minutes,enabled,version) ON bookings.presets TO turnesta_runtime;
GRANT UPDATE(title,mode,local_date,starts_local,ends_local,starts_at,ends_at,places,state,version) ON bookings.offers TO turnesta_runtime;
GRANT UPDATE(result) ON bookings.commands TO turnesta_runtime;

CREATE VIEW bookings.offer_details WITH (security_invoker=true) AS
SELECT tenant_id,location_id,specialist_id,id,title,mode,to_char(local_date,'YYYY-MM-DD') AS date,
    to_char(starts_local,'YYYY-MM-DD"T"HH24:MI') AS starts_local,
    to_char(ends_local,'YYYY-MM-DD"T"HH24:MI') AS ends_local,
    to_char(starts_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"') AS starts_at,
    to_char(ends_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"') AS ends_at,
    time_zone,places,state,version
FROM bookings.offers;
GRANT SELECT ON bookings.offer_details TO turnesta_runtime;

CREATE FUNCTION bookings.valid_date(p_value text) RETURNS date
LANGUAGE plpgsql IMMUTABLE SECURITY INVOKER SET search_path=pg_catalog AS $function$
DECLARE parsed date;
BEGIN
    IF p_value IS NULL THEN RETURN NULL; END IF;
    IF p_value !~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$' THEN RAISE EXCEPTION 'invalid date' USING ERRCODE='B2001'; END IF;
    BEGIN parsed:=p_value::date;
    EXCEPTION WHEN datetime_field_overflow OR invalid_datetime_format THEN
        RAISE EXCEPTION 'invalid date' USING ERRCODE='B2001';
    END;
    IF to_char(parsed,'YYYY-MM-DD')<>p_value THEN RAISE EXCEPTION 'invalid date' USING ERRCODE='B2001'; END IF;
    RETURN parsed;
END
$function$;

-- Use the same timezone database as Core/PostgreSQL. Enumerate observed offsets
-- around the requested wall time, then round-trip candidate instants. A gap has
-- zero candidates, a DST fold has multiple; neither is silently shifted.
CREATE FUNCTION bookings.resolve_local(p_zone text,p_value text) RETURNS timestamptz
LANGUAGE plpgsql STABLE SECURITY INVOKER SET search_path=pg_catalog AS $function$
DECLARE wall timestamp; matches timestamptz[];
BEGIN
    IF p_value IS NULL THEN RETURN NULL; END IF;
    IF p_value !~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}$' THEN RAISE EXCEPTION 'invalid time' USING ERRCODE='B2001'; END IF;
    BEGIN wall:=p_value::timestamp;
    EXCEPTION WHEN datetime_field_overflow OR invalid_datetime_format THEN
        RAISE EXCEPTION 'invalid time' USING ERRCODE='B2001';
    END;
    IF to_char(wall,'YYYY-MM-DD"T"HH24:MI')<>p_value THEN RAISE EXCEPTION 'invalid time' USING ERRCODE='B2001'; END IF;
    SELECT array_agg(candidate ORDER BY candidate) INTO matches FROM (
        SELECT DISTINCT (wall-((probe AT TIME ZONE p_zone)-(probe AT TIME ZONE 'UTC'))) AT TIME ZONE 'UTC' AS candidate
        FROM generate_series((wall AT TIME ZONE 'UTC')-interval '2 days',(wall AT TIME ZONE 'UTC')+interval '2 days',interval '1 hour') probe
    ) choices WHERE candidate AT TIME ZONE p_zone=wall;
    IF coalesce(cardinality(matches),0)=0 THEN RAISE EXCEPTION 'nonexistent local time' USING ERRCODE='B2002'; END IF;
    IF cardinality(matches)<>1 THEN RAISE EXCEPTION 'ambiguous local time' USING ERRCODE='B2003'; END IF;
    RETURN matches[1];
END
$function$;

CREATE FUNCTION bookings.times_conflict(a_start timestamptz,a_end timestamptz,b_start timestamptz,b_end timestamptz)
RETURNS boolean LANGUAGE sql IMMUTABLE SECURITY INVOKER SET search_path=pg_catalog AS $function$
SELECT CASE
    WHEN a_start IS NULL OR b_start IS NULL THEN false
    WHEN a_end IS NULL AND b_end IS NULL THEN a_start=b_start
    WHEN a_end IS NULL THEN a_start>=b_start AND a_start<b_end
    WHEN b_end IS NULL THEN b_start>=a_start AND b_start<a_end
    ELSE a_start<b_end AND b_start<a_end
END
$function$;
REVOKE ALL ON ALL FUNCTIONS IN SCHEMA bookings FROM PUBLIC;
GRANT EXECUTE ON FUNCTION bookings.valid_date(text),bookings.resolve_local(text,text),bookings.times_conflict(timestamptz,timestamptz,timestamptz,timestamptz) TO turnesta_runtime;
