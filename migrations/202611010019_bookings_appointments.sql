-- A separate lock row lets operators serialize reservations without gaining
-- permission to change specialist profiles or the manager-only offer journal.
CREATE TABLE bookings.calendars (
    tenant_id uuid NOT NULL,
    location_id uuid NOT NULL,
    specialist_id uuid NOT NULL,
    revision bigint NOT NULL DEFAULT 1,
    PRIMARY KEY (tenant_id,location_id,specialist_id),
    FOREIGN KEY (tenant_id,location_id,specialist_id) REFERENCES bookings.specialists(tenant_id,location_id,id)
);
ALTER TABLE bookings.calendars ENABLE ROW LEVEL SECURITY;
ALTER TABLE bookings.calendars FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_read ON bookings.calendars FOR SELECT TO turnesta_runtime USING (
    tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
    AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id=calendars.tenant_id
        AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid));
CREATE POLICY staff_write ON bookings.calendars FOR UPDATE TO turnesta_runtime USING (
    tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
    AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id=calendars.tenant_id
        AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.role IN ('owner','manager','operator')));
CREATE POLICY manager_insert ON bookings.calendars FOR INSERT TO turnesta_runtime WITH CHECK (
    tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
    AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id=calendars.tenant_id
        AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.role IN ('owner','manager')));
-- Temporary migration-owner access solely for the existing profiles' backfill.
CREATE POLICY calendar_backfill_read ON bookings.specialists FOR SELECT TO CURRENT_USER USING (true);
CREATE POLICY calendar_backfill_insert ON bookings.calendars FOR INSERT TO CURRENT_USER WITH CHECK (true);
INSERT INTO bookings.calendars(tenant_id,location_id,specialist_id) SELECT tenant_id,location_id,id FROM bookings.specialists;
DROP POLICY calendar_backfill_read ON bookings.specialists;
DROP POLICY calendar_backfill_insert ON bookings.calendars;
GRANT SELECT,INSERT ON bookings.calendars TO turnesta_runtime;
GRANT UPDATE(revision) ON bookings.calendars TO turnesta_runtime;
CREATE FUNCTION bookings.create_calendar() RETURNS trigger LANGUAGE plpgsql SECURITY INVOKER SET search_path=pg_catalog AS $f$
BEGIN
    INSERT INTO bookings.calendars(tenant_id,location_id,specialist_id) VALUES(NEW.tenant_id,NEW.location_id,NEW.id);
    RETURN NEW;
END $f$;
CREATE TRIGGER specialist_calendar AFTER INSERT ON bookings.specialists FOR EACH ROW EXECUTE FUNCTION bookings.create_calendar();

ALTER TABLE bookings.offers ADD CONSTRAINT offers_appointment_scope UNIQUE(tenant_id,location_id,specialist_id,id);
CREATE TABLE bookings.appointments (
    tenant_id uuid NOT NULL,
    location_id uuid NOT NULL,
    specialist_id uuid NOT NULL,
    offer_id uuid NOT NULL,
    id uuid NOT NULL,
    client_name text NOT NULL CHECK(char_length(client_name) BETWEEN 1 AND 120 AND client_name=btrim(client_name) AND client_name !~ '[[:cntrl:]]'),
    phone text NOT NULL CHECK(char_length(phone)<=64 AND phone=btrim(phone) AND phone !~ '[[:cntrl:]]'),
    email text NOT NULL CHECK(char_length(email)<=254 AND email=btrim(email) AND email !~ '[[:cntrl:]]'),
    note text NOT NULL CHECK(char_length(note)<=1000),
    status text NOT NULL CHECK(status IN ('confirmed','cancelled')),
    source text NOT NULL DEFAULT 'staff' CHECK(source='staff'),
    title text NOT NULL,
    specialist_name text NOT NULL,
    mode text NOT NULL CHECK(mode IN ('interval','start_only','by_agreement')),
    local_date date,
    starts_local timestamp,
    ends_local timestamp,
    starts_at timestamptz,
    ends_at timestamptz,
    time_zone text NOT NULL REFERENCES core.time_zones(name),
    created_by uuid NOT NULL REFERENCES core.users(id),
    updated_by uuid NOT NULL REFERENCES core.users(id),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    cancelled_at timestamptz,
    version integer NOT NULL DEFAULT 1 CHECK(version>0),
    PRIMARY KEY(tenant_id,id),
    FOREIGN KEY(tenant_id,location_id,specialist_id,offer_id) REFERENCES bookings.offers(tenant_id,location_id,specialist_id,id),
    CHECK ((status='cancelled')=(cancelled_at IS NOT NULL))
);
CREATE INDEX appointments_offer_idx ON bookings.appointments(tenant_id,offer_id,id) WHERE status='confirmed';
CREATE INDEX appointments_time_idx ON bookings.appointments(tenant_id,specialist_id,starts_at) WHERE status='confirmed';
CREATE INDEX appointments_day_idx ON bookings.appointments(tenant_id,location_id,specialist_id,local_date,id);
CREATE TABLE bookings.appointment_commands (
    tenant_id uuid NOT NULL,
    location_id uuid NOT NULL,
    actor_id uuid NOT NULL,
    idempotency_key uuid NOT NULL,
    fingerprint bytea NOT NULL CHECK(octet_length(fingerprint)=32),
    result jsonb CHECK(result IS NULL OR jsonb_typeof(result)='object'),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(tenant_id,location_id,actor_id,idempotency_key),
    FOREIGN KEY(tenant_id,location_id) REFERENCES core.locations(tenant_id,id),
    FOREIGN KEY(tenant_id,actor_id) REFERENCES core.memberships(tenant_id,user_id)
);
DO $policies$
DECLARE target text;
BEGIN
    FOREACH target IN ARRAY ARRAY['appointments','appointment_commands'] LOOP
        EXECUTE format('ALTER TABLE bookings.%I ENABLE ROW LEVEL SECURITY',target);
        EXECUTE format('ALTER TABLE bookings.%I FORCE ROW LEVEL SECURITY',target);
        EXECUTE format($sql$CREATE POLICY staff_access ON bookings.%I TO turnesta_runtime USING (
            tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
            AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id=%1$I.tenant_id
                AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.role IN ('owner','manager','operator')))
            WITH CHECK (tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
            AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id=%1$I.tenant_id
                AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.role IN ('owner','manager','operator')))$sql$,target);
    END LOOP;
END $policies$;
CREATE POLICY own_command ON bookings.appointment_commands AS RESTRICTIVE TO turnesta_runtime
    USING(actor_id=nullif(current_setting('app.user_id',true),'')::uuid)
    WITH CHECK(actor_id=nullif(current_setting('app.user_id',true),'')::uuid);
GRANT SELECT,INSERT ON bookings.appointments,bookings.appointment_commands TO turnesta_runtime;
GRANT UPDATE(offer_id,client_name,phone,email,note,status,version) ON bookings.appointments TO turnesta_runtime;
GRANT UPDATE(result) ON bookings.appointment_commands TO turnesta_runtime;

CREATE FUNCTION bookings.guard_appointment() RETURNS trigger LANGUAGE plpgsql SECURITY INVOKER SET search_path=pg_catalog AS $f$
DECLARE target bookings.offers%ROWTYPE; actor uuid; new_place boolean;
BEGIN
    actor:=nullif(current_setting('app.user_id',true),'')::uuid;
    IF NEW.tenant_id IS DISTINCT FROM nullif(current_setting('app.tenant_id',true),'')::uuid OR NOT EXISTS (
        SELECT 1 FROM core.memberships m WHERE m.tenant_id=NEW.tenant_id AND m.user_id=actor AND m.role IN ('owner','manager','operator'))
    THEN RAISE EXCEPTION 'forbidden' USING ERRCODE='42501'; END IF;
    PERFORM 1 FROM bookings.calendars WHERE tenant_id=NEW.tenant_id AND location_id=NEW.location_id AND specialist_id=NEW.specialist_id FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'calendar not found' USING ERRCODE='B2101'; END IF;
    IF TG_OP='UPDATE' THEN
        IF (NEW.tenant_id,NEW.location_id,NEW.specialist_id,NEW.id) IS DISTINCT FROM (OLD.tenant_id,OLD.location_id,OLD.specialist_id,OLD.id)
            OR NEW.version<>OLD.version+1 OR OLD.status='cancelled'
        THEN RAISE EXCEPTION 'invalid appointment transition' USING ERRCODE='B2102'; END IF;
        NEW.created_by:=OLD.created_by; NEW.created_at:=OLD.created_at; NEW.source:=OLD.source;
        new_place:=NEW.offer_id<>OLD.offer_id;
    ELSE
        IF NEW.status<>'confirmed' OR NEW.version<>1 THEN RAISE EXCEPTION 'invalid initial appointment' USING ERRCODE='B2102'; END IF;
        NEW.created_by:=actor; NEW.created_at:=clock_timestamp();
        new_place:=true;
    END IF;
    IF new_place THEN
        SELECT * INTO target FROM bookings.offers WHERE tenant_id=NEW.tenant_id AND location_id=NEW.location_id AND specialist_id=NEW.specialist_id AND id=NEW.offer_id;
        IF NOT FOUND THEN RAISE EXCEPTION 'offer not found' USING ERRCODE='B2101'; END IF;
        IF NEW.status<>'confirmed' OR target.state<>'ready' OR NOT EXISTS (
            SELECT 1 FROM bookings.specialists WHERE tenant_id=NEW.tenant_id AND id=NEW.specialist_id AND enabled)
        THEN RAISE EXCEPTION 'offer unavailable' USING ERRCODE='B2103'; END IF;
        IF (SELECT count(*) FROM bookings.appointments WHERE tenant_id=NEW.tenant_id AND offer_id=NEW.offer_id AND status='confirmed' AND id<>NEW.id)>=target.places
        THEN RAISE EXCEPTION 'offer full' USING ERRCODE='B2104'; END IF;
        IF EXISTS(SELECT 1 FROM bookings.appointments a WHERE a.tenant_id=NEW.tenant_id AND a.specialist_id=NEW.specialist_id
            AND a.status='confirmed' AND a.id<>NEW.id AND a.offer_id<>NEW.offer_id
            AND bookings.times_conflict(a.starts_at,a.ends_at,target.starts_at,target.ends_at))
        THEN RAISE EXCEPTION 'specialist busy' USING ERRCODE='B2105'; END IF;
        NEW.title:=target.title; NEW.mode:=target.mode; NEW.local_date:=target.local_date;
        NEW.starts_local:=target.starts_local; NEW.ends_local:=target.ends_local;
        NEW.starts_at:=target.starts_at; NEW.ends_at:=target.ends_at; NEW.time_zone:=target.time_zone;
        SELECT name INTO NEW.specialist_name FROM bookings.specialists WHERE tenant_id=NEW.tenant_id AND id=NEW.specialist_id;
    END IF;
    NEW.updated_by:=actor; NEW.updated_at:=clock_timestamp();
    NEW.cancelled_at:=CASE WHEN NEW.status='cancelled' THEN clock_timestamp() ELSE NULL END;
    UPDATE bookings.calendars SET revision=revision+1 WHERE tenant_id=NEW.tenant_id AND location_id=NEW.location_id AND specialist_id=NEW.specialist_id;
    RETURN NEW;
END $f$;
CREATE TRIGGER appointment_guard BEFORE INSERT OR UPDATE ON bookings.appointments FOR EACH ROW EXECUTE FUNCTION bookings.guard_appointment();

CREATE FUNCTION bookings.guard_occupied_offer() RETURNS trigger LANGUAGE plpgsql SECURITY INVOKER SET search_path=pg_catalog AS $f$
DECLARE occupied bigint;
BEGIN
    PERFORM 1 FROM bookings.calendars WHERE tenant_id=NEW.tenant_id AND location_id=NEW.location_id AND specialist_id=NEW.specialist_id FOR UPDATE;
    SELECT count(*) INTO occupied FROM bookings.appointments WHERE tenant_id=OLD.tenant_id AND offer_id=OLD.id AND status='confirmed';
    IF occupied>0 AND ((NEW.mode,NEW.local_date,NEW.starts_local,NEW.ends_local,NEW.starts_at,NEW.ends_at,NEW.time_zone)
        IS DISTINCT FROM (OLD.mode,OLD.local_date,OLD.starts_local,OLD.ends_local,OLD.starts_at,OLD.ends_at,OLD.time_zone) OR NEW.places<occupied)
    THEN RAISE EXCEPTION 'occupied offer requires explicit rescheduling' USING ERRCODE='B2106'; END IF;
    RETURN NEW;
END $f$;
CREATE TRIGGER occupied_offer_guard BEFORE UPDATE ON bookings.offers FOR EACH ROW EXECUTE FUNCTION bookings.guard_occupied_offer();

CREATE VIEW bookings.appointment_details WITH(security_invoker=true) AS
SELECT tenant_id,location_id,specialist_id,offer_id,id,client_name,phone,email,note,status,source,title,specialist_name,mode,
    to_char(local_date,'YYYY-MM-DD') AS date,to_char(starts_local,'YYYY-MM-DD"T"HH24:MI') AS starts_local,
    to_char(ends_local,'YYYY-MM-DD"T"HH24:MI') AS ends_local,
    to_char(starts_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"') AS starts_at,
    to_char(ends_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"') AS ends_at,time_zone,
    created_by,updated_by,to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS created_at,
    to_char(updated_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS updated_at,
    to_char(cancelled_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS cancelled_at,version
FROM bookings.appointments;
GRANT SELECT ON bookings.appointment_details TO turnesta_runtime;
REVOKE ALL ON FUNCTION bookings.create_calendar(),bookings.guard_appointment(),bookings.guard_occupied_offer() FROM PUBLIC;
