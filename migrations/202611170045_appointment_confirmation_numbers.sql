-- Number every existing visit without changing its business state or revision.
ALTER TABLE bookings.appointments ADD COLUMN confirmation_number bigint;
ALTER TABLE bookings.appointments DISABLE TRIGGER appointment_guard;
WITH numbered AS (
    SELECT tenant_id,id,row_number() OVER (PARTITION BY tenant_id ORDER BY created_at,id) AS number
    FROM bookings.appointments
)
UPDATE bookings.appointments a SET confirmation_number=n.number
FROM numbered n WHERE a.tenant_id=n.tenant_id AND a.id=n.id;
ALTER TABLE bookings.appointments ENABLE TRIGGER appointment_guard;
ALTER TABLE bookings.appointments ALTER COLUMN confirmation_number SET NOT NULL;
ALTER TABLE bookings.appointments ADD CONSTRAINT appointment_number_positive CHECK(confirmation_number>0);
ALTER TABLE bookings.appointments ADD CONSTRAINT appointment_number_unique UNIQUE(tenant_id,confirmation_number);

CREATE TABLE bookings.confirmation_counters (
    tenant_id uuid PRIMARY KEY REFERENCES core.organizations(id),
    last_number bigint NOT NULL CHECK(last_number>0)
);
INSERT INTO bookings.confirmation_counters(tenant_id,last_number)
SELECT tenant_id,max(confirmation_number) FROM bookings.appointments GROUP BY tenant_id;
ALTER TABLE bookings.confirmation_counters ENABLE ROW LEVEL SECURITY;
ALTER TABLE bookings.confirmation_counters FORCE ROW LEVEL SECURITY;
CREATE POLICY counter_owner ON bookings.confirmation_counters TO CURRENT_USER USING(true) WITH CHECK(true);
REVOKE ALL ON bookings.confirmation_counters FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;

CREATE FUNCTION bookings.assign_confirmation_number() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $f$
BEGIN
    IF TG_OP='INSERT' THEN
        INSERT INTO bookings.confirmation_counters(tenant_id,last_number) VALUES(NEW.tenant_id,1)
        ON CONFLICT(tenant_id) DO UPDATE SET last_number=bookings.confirmation_counters.last_number+1
        RETURNING last_number INTO NEW.confirmation_number;
    ELSIF NEW.confirmation_number IS DISTINCT FROM OLD.confirmation_number THEN
        RAISE EXCEPTION 'appointment number is immutable' USING ERRCODE='B2102';
    END IF;
    RETURN NEW;
END $f$;
REVOKE ALL ON FUNCTION bookings.assign_confirmation_number() FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;
-- Runs after appointment_guard: the calendar and the actor are checked first.
CREATE TRIGGER appointment_number BEFORE INSERT OR UPDATE ON bookings.appointments
FOR EACH ROW EXECUTE FUNCTION bookings.assign_confirmation_number();

CREATE OR REPLACE VIEW bookings.appointment_details WITH(security_invoker=true) AS
SELECT tenant_id,location_id,specialist_id,offer_id,id,client_name,phone,email,note,status,source,title,specialist_name,mode,
    to_char(local_date,'YYYY-MM-DD') AS date,to_char(starts_local,'YYYY-MM-DD"T"HH24:MI') AS starts_local,
    to_char(ends_local,'YYYY-MM-DD"T"HH24:MI') AS ends_local,
    to_char(starts_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"') AS starts_at,
    to_char(ends_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"') AS ends_at,time_zone,
    created_by,updated_by,to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS created_at,
    to_char(updated_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS updated_at,
    to_char(cancelled_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS cancelled_at,version,form_snapshot,answers,
    to_char(anonymized_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS anonymized_at,
    lpad(confirmation_number::text,greatest(6,length(confirmation_number::text)),'0') AS confirmation_number
FROM bookings.appointments;

CREATE OR REPLACE FUNCTION bookings.public_receipt(p_slug uuid,p_hash bytea) RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE receipt jsonb; expiry timestamptz;
BEGIN
    IF octet_length(p_hash) IS DISTINCT FROM 32 THEN RAISE EXCEPTION 'invalid access' USING ERRCODE='B2204'; END IF;
    SELECT c.expires_at,jsonb_build_object('reference',c.reference,
        'confirmation_number',lpad(a.confirmation_number::text,greatest(6,length(a.confirmation_number::text)),'0'),
        'status',a.status,'title',a.title,'specialist_name',a.specialist_name,'time_zone',c.public_zone,
        'organization_name',org.name,'location_name',l.name,'version',a.version,
        'date',CASE WHEN a.starts_at IS NULL THEN to_char(a.local_date,'YYYY-MM-DD') ELSE to_char(a.starts_at AT TIME ZONE c.public_zone,'YYYY-MM-DD') END,
        'starts_local',to_char(a.starts_at AT TIME ZONE c.public_zone,'YYYY-MM-DD"T"HH24:MI'),
        'ends_local',to_char(a.ends_at AT TIME ZONE c.public_zone,'YYYY-MM-DD"T"HH24:MI'),'starts_at',to_char(a.starts_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"'),'ends_at',to_char(a.ends_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"'))
    INTO expiry,receipt FROM bookings.public_claims c JOIN bookings.public_settings s ON s.tenant_id=c.tenant_id AND s.location_id=c.location_id
        JOIN bookings.appointments a ON a.tenant_id=c.tenant_id AND a.id=c.appointment_id
        JOIN core.locations l ON l.tenant_id=c.tenant_id AND l.id=c.location_id JOIN core.organizations org ON org.id=c.tenant_id
        WHERE s.public_slug=p_slug AND c.token_hash=p_hash;
    IF NOT FOUND THEN
        IF EXISTS(SELECT 1 FROM bookings.public_settings s JOIN retention.used_keys t ON t.kind='bookings.public_claims' AND t.tenant_id=s.tenant_id AND t.scope_id=s.location_id AND t.actor_id='' AND t.key_value=to_jsonb(p_hash)#>>'{}' WHERE s.public_slug=p_slug) THEN
            RAISE EXCEPTION 'receipt expired' USING ERRCODE='B2207';
        END IF;
        RAISE EXCEPTION 'receipt not found' USING ERRCODE='B2201';
    END IF;
    IF expiry<=current_timestamp THEN RAISE EXCEPTION 'receipt expired' USING ERRCODE='B2207'; END IF;
    RETURN receipt;
END $f$;
REVOKE ALL ON FUNCTION bookings.public_receipt(uuid,bytea) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION bookings.public_receipt(uuid,bytea) TO turnesta_runtime;
