CREATE FUNCTION bookings.form_valid(p_form jsonb) RETURNS boolean LANGUAGE plpgsql IMMUTABLE SECURITY INVOKER SET search_path=pg_catalog AS $f$
DECLARE field jsonb; option jsonb; keys text[]:=ARRAY[]::text[]; field_key text; kind text; found_name boolean:=false;
BEGIN
    IF p_form IS NULL OR jsonb_typeof(p_form) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
    IF (SELECT count(*) FROM jsonb_object_keys(p_form))<>1 OR jsonb_typeof(p_form->'fields') IS DISTINCT FROM 'array' THEN RETURN false; END IF;
    IF jsonb_array_length(p_form->'fields') NOT BETWEEN 1 AND 14 THEN RETURN false; END IF;
    FOR field IN SELECT value FROM jsonb_array_elements(p_form->'fields') LOOP
        IF jsonb_typeof(field) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
        IF NOT field ?& ARRAY['key','label','kind','required','options'] OR (SELECT count(*) FROM jsonb_object_keys(field))<>5 THEN RETURN false; END IF;
        IF jsonb_typeof(field->'key') IS DISTINCT FROM 'string' OR jsonb_typeof(field->'label') IS DISTINCT FROM 'string'
            OR jsonb_typeof(field->'kind') IS DISTINCT FROM 'string' OR jsonb_typeof(field->'required') IS DISTINCT FROM 'boolean'
            OR jsonb_typeof(field->'options') IS DISTINCT FROM 'array' THEN RETURN false; END IF;
        field_key:=field->>'key'; kind:=field->>'kind';
        IF field_key=ANY(keys) OR field_key !~ '^[a-z][a-z0-9_]{0,39}$' OR char_length(field->>'label') NOT BETWEEN 1 AND 120
            OR field->>'label'<>btrim(field->>'label') OR field->>'label' ~ '[[:cntrl:]]' THEN RETURN false; END IF;
        keys:=array_append(keys,field_key);
        IF field_key IN ('client_name','phone','email','note') THEN
            IF kind<>field_key THEN RETURN false; END IF;
        ELSIF field_key !~ '^field_[a-z0-9_]+$' OR kind NOT IN ('text','select') THEN RETURN false; END IF;
        IF field_key='client_name' THEN
            IF field->>'required'<>'true' THEN RETURN false; END IF;
            found_name:=true;
        END IF;
        IF kind='select' THEN
            IF jsonb_array_length(field->'options') NOT BETWEEN 1 AND 10 OR (SELECT count(DISTINCT value) FROM jsonb_array_elements(field->'options'))<>jsonb_array_length(field->'options') THEN RETURN false; END IF;
            FOR option IN SELECT value FROM jsonb_array_elements(field->'options') LOOP
                IF jsonb_typeof(option) IS DISTINCT FROM 'string' THEN RETURN false; END IF;
                IF char_length(option#>>'{}') NOT BETWEEN 1 AND 120 OR option#>>'{}'<>btrim(option#>>'{}') OR option#>>'{}' ~ '[[:cntrl:]]' THEN RETURN false; END IF;
            END LOOP;
        ELSIF jsonb_array_length(field->'options')<>0 THEN RETURN false;
        END IF;
    END LOOP;
    RETURN found_name;
END $f$;

CREATE FUNCTION bookings.clean_answers(p_answers jsonb) RETURNS jsonb LANGUAGE plpgsql IMMUTABLE SECURITY INVOKER SET search_path=pg_catalog AS $f$
DECLARE entry record; answer text; result jsonb:='{}';
BEGIN
    IF p_answers IS NULL OR jsonb_typeof(p_answers) IS DISTINCT FROM 'object' THEN RAISE EXCEPTION 'invalid answers' USING ERRCODE='B2204'; END IF;
    IF (SELECT count(*) FROM jsonb_object_keys(p_answers))>14 THEN RAISE EXCEPTION 'too many answers' USING ERRCODE='B2204'; END IF;
    FOR entry IN SELECT * FROM jsonb_each(p_answers) LOOP
        IF jsonb_typeof(entry.value) IS DISTINCT FROM 'string' OR entry.key !~ '^[a-z][a-z0-9_]{0,39}$' THEN RAISE EXCEPTION 'invalid answer' USING ERRCODE='B2204'; END IF;
        answer:=btrim(replace(entry.value#>>'{}',E'\r\n',E'\n'),E' \t\n\r\v\f');
        IF char_length(answer)>1000 OR translate(answer,E'\n\t','') ~ '[[:cntrl:]]' THEN RAISE EXCEPTION 'invalid answer' USING ERRCODE='B2204'; END IF;
        IF entry.key='email' THEN answer:=lower(answer); END IF;
        result:=result||jsonb_build_object(entry.key,answer);
    END LOOP;
    RETURN result;
END $f$;

CREATE FUNCTION bookings.validate_answers(p_form jsonb,p_answers jsonb) RETURNS void LANGUAGE plpgsql IMMUTABLE SECURITY INVOKER SET search_path=pg_catalog AS $f$
DECLARE field jsonb; answer text; kind text; field_key text;
BEGIN
    IF NOT bookings.form_valid(p_form) THEN RAISE EXCEPTION 'invalid form' USING ERRCODE='B2204'; END IF;
    IF EXISTS(SELECT 1 FROM jsonb_object_keys(p_answers) AS keys(key) WHERE NOT EXISTS(SELECT 1 FROM jsonb_array_elements(p_form->'fields') f WHERE f->>'key'=keys.key)) THEN RAISE EXCEPTION 'unknown answer' USING ERRCODE='B2204'; END IF;
    FOR field IN SELECT value FROM jsonb_array_elements(p_form->'fields') LOOP
        field_key:=field->>'key'; kind:=field->>'kind'; answer:=coalesce(p_answers->>field_key,'');
        IF field->>'required'='true' AND answer='' THEN RAISE EXCEPTION 'required answer missing' USING ERRCODE='B2204'; END IF;
        IF answer='' THEN CONTINUE; END IF;
        IF kind IN ('client_name','text','select') AND (char_length(answer)>CASE WHEN kind='text' THEN 500 ELSE 120 END OR answer ~ '[[:cntrl:]]') THEN RAISE EXCEPTION 'invalid text' USING ERRCODE='B2204'; END IF;
        IF kind='phone' AND (char_length(answer)>64 OR answer !~ '^[0-9+() .#xX-]+$' OR char_length(regexp_replace(answer,'[^0-9]','','g'))<3) THEN RAISE EXCEPTION 'invalid phone' USING ERRCODE='B2204'; END IF;
        IF kind='email' AND (octet_length(answer)>254 OR answer !~ '^[^[:space:]@]+@[^[:space:]@]+\.[^[:space:]@]+$') THEN RAISE EXCEPTION 'invalid email' USING ERRCODE='B2204'; END IF;
        IF kind='select' AND NOT (field->'options') ? answer THEN RAISE EXCEPTION 'unknown choice' USING ERRCODE='B2204'; END IF;
    END LOOP;
END $f$;

CREATE TABLE bookings.public_settings (
    tenant_id uuid NOT NULL,
    location_id uuid NOT NULL,
    public_slug uuid NOT NULL UNIQUE,
    enabled boolean NOT NULL,
    form jsonb NOT NULL CHECK(bookings.form_valid(form)),
    version integer NOT NULL DEFAULT 1 CHECK(version>0),
    PRIMARY KEY(tenant_id,location_id),
    FOREIGN KEY(tenant_id,location_id) REFERENCES core.locations(tenant_id,id)
);
ALTER TABLE bookings.public_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE bookings.public_settings FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_read ON bookings.public_settings FOR SELECT TO turnesta_runtime USING (
    tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid AND EXISTS(SELECT 1 FROM core.memberships m WHERE m.tenant_id=public_settings.tenant_id AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid));
CREATE POLICY manager_insert ON bookings.public_settings FOR INSERT TO turnesta_runtime WITH CHECK (
    tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid AND EXISTS(SELECT 1 FROM core.memberships m WHERE m.tenant_id=public_settings.tenant_id AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.role IN ('owner','manager')));
CREATE POLICY manager_update ON bookings.public_settings FOR UPDATE TO turnesta_runtime USING (
    tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid AND EXISTS(SELECT 1 FROM core.memberships m WHERE m.tenant_id=public_settings.tenant_id AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.role IN ('owner','manager')));
GRANT SELECT,INSERT ON bookings.public_settings TO turnesta_runtime;
GRANT UPDATE(enabled,form,version) ON bookings.public_settings TO turnesta_runtime;

ALTER TABLE bookings.appointments DROP CONSTRAINT appointments_source_check;
ALTER TABLE bookings.appointments ALTER COLUMN created_by DROP NOT NULL;
ALTER TABLE bookings.appointments ALTER COLUMN updated_by DROP NOT NULL;
ALTER TABLE bookings.appointments ADD CONSTRAINT appointments_source_check CHECK(source IN ('staff','public'));
ALTER TABLE bookings.appointments ADD COLUMN form_snapshot jsonb;
ALTER TABLE bookings.appointments ADD COLUMN answers jsonb NOT NULL DEFAULT '{}' CHECK(jsonb_typeof(answers)='object');
ALTER TABLE bookings.appointments ADD CONSTRAINT public_form_snapshot CHECK(source<>'public' OR (form_snapshot IS NOT NULL AND bookings.form_valid(form_snapshot)));
ALTER TABLE bookings.appointments ADD CONSTRAINT appointment_public_claim_scope UNIQUE(tenant_id,location_id,id);
CREATE TABLE bookings.public_claims (
    tenant_id uuid NOT NULL,
    location_id uuid NOT NULL,
    token_hash bytea NOT NULL CHECK(octet_length(token_hash)=32),
    appointment_id uuid NOT NULL,
    reference uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    request_fingerprint bytea NOT NULL CHECK(octet_length(request_fingerprint)=32),
    public_zone text NOT NULL REFERENCES core.time_zones(name),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    expires_at timestamptz NOT NULL DEFAULT clock_timestamp()+interval '400 days',
    PRIMARY KEY(tenant_id,location_id,token_hash),
    UNIQUE(tenant_id,appointment_id),
    FOREIGN KEY(tenant_id,location_id) REFERENCES bookings.public_settings(tenant_id,location_id),
    FOREIGN KEY(tenant_id,location_id,appointment_id) REFERENCES bookings.appointments(tenant_id,location_id,id)
);
CREATE INDEX public_claims_budget_idx ON bookings.public_claims(tenant_id,location_id,created_at);
CREATE INDEX offers_public_day_idx ON bookings.offers(tenant_id,location_id,starts_at,id) WHERE state='ready';
ALTER TABLE bookings.public_claims ENABLE ROW LEVEL SECURITY;
ALTER TABLE bookings.public_claims FORCE ROW LEVEL SECURITY;
REVOKE ALL ON bookings.public_claims FROM PUBLIC,turnesta_runtime,turnesta_provisioner;

-- Only the exact migration/function owner can use these internal policies.
-- No table privileges are added to runtime, provisioner, or worker roles.
CREATE POLICY public_owner_read ON bookings.specialists FOR SELECT TO CURRENT_USER USING(true);
CREATE POLICY public_owner_read ON bookings.offers FOR SELECT TO CURRENT_USER USING(true);
CREATE POLICY public_owner_access ON bookings.calendars TO CURRENT_USER USING(true) WITH CHECK(true);
CREATE POLICY public_owner_access ON bookings.appointments TO CURRENT_USER USING(true) WITH CHECK(true);
CREATE POLICY public_owner_access ON bookings.public_settings TO CURRENT_USER USING(true) WITH CHECK(true);
CREATE POLICY public_owner_access ON bookings.public_claims TO CURRENT_USER USING(true) WITH CHECK(true);

CREATE FUNCTION bookings.public_profile(p_slug uuid) RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $f$
SELECT jsonb_build_object('public_slug',s.public_slug,'organization_name',o.name,'location_name',l.name,'time_zone',l.time_zone,
    'today',to_char(current_timestamp AT TIME ZONE l.time_zone,'YYYY-MM-DD'),'enabled',s.enabled,'version',s.version,
    'form',CASE WHEN s.enabled THEN s.form ELSE NULL END,'specialists',CASE WHEN s.enabled THEN coalesce((SELECT jsonb_agg(jsonb_build_object('id',p.id,'name',p.name,'description',p.description,'version',p.version) ORDER BY p.name,p.id)
        FROM bookings.specialists p WHERE p.tenant_id=s.tenant_id AND p.location_id=s.location_id AND p.enabled),'[]') ELSE '[]'::jsonb END)
FROM bookings.public_settings s JOIN core.locations l ON l.tenant_id=s.tenant_id AND l.id=s.location_id JOIN core.organizations o ON o.id=s.tenant_id WHERE s.public_slug=p_slug
$f$;

CREATE FUNCTION bookings.public_receipt(p_slug uuid,p_hash bytea) RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE receipt jsonb; expiry timestamptz;
BEGIN
    IF octet_length(p_hash) IS DISTINCT FROM 32 THEN RAISE EXCEPTION 'invalid access' USING ERRCODE='B2204'; END IF;
    SELECT c.expires_at,jsonb_build_object('reference',c.reference,'status',a.status,'title',a.title,'specialist_name',a.specialist_name,'time_zone',c.public_zone,
        'organization_name',org.name,'location_name',l.name,'version',a.version,
        'date',CASE WHEN a.starts_at IS NULL THEN to_char(a.local_date,'YYYY-MM-DD') ELSE to_char(a.starts_at AT TIME ZONE c.public_zone,'YYYY-MM-DD') END,
        'starts_local',to_char(a.starts_at AT TIME ZONE c.public_zone,'YYYY-MM-DD"T"HH24:MI'),
        'ends_local',to_char(a.ends_at AT TIME ZONE c.public_zone,'YYYY-MM-DD"T"HH24:MI'),'starts_at',to_char(a.starts_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"'),'ends_at',to_char(a.ends_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"'))
    INTO expiry,receipt FROM bookings.public_claims c JOIN bookings.public_settings s ON s.tenant_id=c.tenant_id AND s.location_id=c.location_id
        JOIN bookings.appointments a ON a.tenant_id=c.tenant_id AND a.id=c.appointment_id
        JOIN core.locations l ON l.tenant_id=c.tenant_id AND l.id=c.location_id JOIN core.organizations org ON org.id=c.tenant_id
        WHERE s.public_slug=p_slug AND c.token_hash=p_hash;
    IF NOT FOUND THEN RAISE EXCEPTION 'receipt not found' USING ERRCODE='B2201'; END IF;
    IF expiry<=current_timestamp THEN RAISE EXCEPTION 'receipt expired' USING ERRCODE='B2207'; END IF;
    RETURN receipt;
END $f$;

CREATE FUNCTION bookings.public_book(p_slug uuid,p_hash bytea,p_offer uuid,p_offer_version integer,p_specialist_version integer,p_form_version integer,p_zone text,p_answers jsonb)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE settings bookings.public_settings%ROWTYPE; target bookings.offers%ROWTYPE; fingerprint bytea; normalized jsonb; previous bytea; appointment uuid:=gen_random_uuid(); point_zone text;
BEGIN
    IF octet_length(p_hash) IS DISTINCT FROM 32 THEN RAISE EXCEPTION 'invalid access' USING ERRCODE='B2204'; END IF;
    normalized:=bookings.clean_answers(p_answers);
    IF p_offer IS NULL OR p_offer_version IS NULL OR p_offer_version<1 OR p_specialist_version IS NULL OR p_specialist_version<1 OR p_form_version IS NULL OR p_form_version<1 OR p_zone IS NULL THEN RAISE EXCEPTION 'invalid booking' USING ERRCODE='B2204'; END IF;
    fingerprint:=sha256(convert_to(jsonb_build_object('offer_id',p_offer,'offer_version',p_offer_version,'specialist_version',p_specialist_version,'form_version',p_form_version,'time_zone',p_zone,'answers',normalized)::text,'UTF8'));
    SELECT * INTO settings FROM bookings.public_settings WHERE public_slug=p_slug FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'booking page not found' USING ERRCODE='B2201'; END IF;
    SELECT request_fingerprint INTO previous FROM bookings.public_claims WHERE tenant_id=settings.tenant_id AND location_id=settings.location_id AND token_hash=p_hash;
    IF FOUND THEN
        IF previous<>fingerprint THEN RAISE EXCEPTION 'different request' USING ERRCODE='B2206'; END IF;
        RETURN bookings.public_receipt(p_slug,p_hash);
    END IF;
    IF NOT settings.enabled THEN RAISE EXCEPTION 'booking paused' USING ERRCODE='B2202'; END IF;
    SELECT time_zone INTO point_zone FROM core.locations WHERE tenant_id=settings.tenant_id AND id=settings.location_id;
    IF settings.version<>p_form_version OR point_zone<>p_zone THEN RAISE EXCEPTION 'review changed form or timezone' USING ERRCODE='B2205'; END IF;
    PERFORM bookings.validate_answers(settings.form,normalized);
    SELECT * INTO target FROM bookings.offers WHERE tenant_id=settings.tenant_id AND location_id=settings.location_id AND id=p_offer;
    IF NOT FOUND THEN RAISE EXCEPTION 'offer unavailable' USING ERRCODE='B2103'; END IF;
    PERFORM 1 FROM bookings.calendars WHERE tenant_id=target.tenant_id AND location_id=target.location_id AND specialist_id=target.specialist_id FOR UPDATE;
    SELECT * INTO target FROM bookings.offers WHERE tenant_id=settings.tenant_id AND location_id=settings.location_id AND id=p_offer;
    IF target.version<>p_offer_version OR NOT EXISTS(SELECT 1 FROM bookings.specialists WHERE tenant_id=target.tenant_id AND id=target.specialist_id AND version=p_specialist_version AND enabled) THEN RAISE EXCEPTION 'offer or specialist changed' USING ERRCODE='B2205'; END IF;
    IF target.state<>'ready' OR (target.starts_at IS NOT NULL AND target.starts_at<=clock_timestamp())
        OR (target.starts_at IS NULL AND target.local_date IS NOT NULL AND target.local_date<(clock_timestamp() AT TIME ZONE point_zone)::date)
        OR coalesce((target.starts_at AT TIME ZONE point_zone)::date,target.local_date)>=(date_trunc('month',clock_timestamp() AT TIME ZONE point_zone)+interval '13 months')::date
    THEN RAISE EXCEPTION 'offer unavailable' USING ERRCODE='B2103'; END IF;
    IF (SELECT count(*) FROM bookings.public_claims WHERE tenant_id=settings.tenant_id AND location_id=settings.location_id AND created_at>clock_timestamp()-interval '1 minute')>=30
        OR (SELECT count(*) FROM bookings.public_claims c JOIN bookings.appointments a ON a.tenant_id=c.tenant_id AND a.id=c.appointment_id WHERE c.tenant_id=settings.tenant_id AND c.location_id=settings.location_id AND a.status='confirmed')>=1000
    THEN RAISE EXCEPTION 'booking rate exceeded' USING ERRCODE='B2203'; END IF;
    INSERT INTO bookings.appointments(tenant_id,location_id,specialist_id,offer_id,id,client_name,phone,email,note,status,source,form_snapshot,answers)
        VALUES(target.tenant_id,target.location_id,target.specialist_id,target.id,appointment,normalized->>'client_name',coalesce(normalized->>'phone',''),coalesce(normalized->>'email',''),coalesce(normalized->>'note',''),'confirmed','public',settings.form,normalized);
    INSERT INTO bookings.public_claims(tenant_id,location_id,token_hash,appointment_id,request_fingerprint,public_zone)
        VALUES(target.tenant_id,target.location_id,p_hash,appointment,fingerprint,point_zone);
    RETURN bookings.public_receipt(p_slug,p_hash);
END $f$;

REVOKE ALL ON FUNCTION bookings.form_valid(jsonb),bookings.clean_answers(jsonb),bookings.validate_answers(jsonb,jsonb),bookings.public_profile(uuid),bookings.public_receipt(uuid,bytea),bookings.public_book(uuid,bytea,uuid,integer,integer,integer,text,jsonb) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION bookings.form_valid(jsonb),bookings.public_profile(uuid),bookings.public_receipt(uuid,bytea),bookings.public_book(uuid,bytea,uuid,integer,integer,integer,text,jsonb) TO turnesta_runtime;

CREATE FUNCTION bookings.public_availability(p_slug uuid,p_month text,p_date text,p_specialist uuid,p_after uuid,p_version text,p_limit integer)
RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE settings bookings.public_settings%ROWTYPE; profile jsonb; zone text; month_start date; chosen_date date; current_month date; result jsonb; snapshot text; cursor_exists boolean;
BEGIN
    SELECT * INTO settings FROM bookings.public_settings WHERE public_slug=p_slug;
    IF NOT FOUND THEN RAISE EXCEPTION 'booking page not found' USING ERRCODE='B2201'; END IF;
    IF NOT settings.enabled THEN RAISE EXCEPTION 'booking paused' USING ERRCODE='B2202'; END IF;
    profile:=bookings.public_profile(p_slug);zone:=profile->>'time_zone';
    month_start:=bookings.valid_date(p_month||'-01');chosen_date:=bookings.valid_date(p_date);
    current_month:=date_trunc('month',current_timestamp AT TIME ZONE zone)::date;
    IF p_month IS NULL OR p_date IS NULL OR p_limit IS NULL OR p_limit NOT BETWEEN 1 AND 100 OR month_start<current_month OR month_start>current_month+interval '12 months'
        OR chosen_date<month_start OR chosen_date>=month_start+interval '1 month' OR (p_after IS NOT NULL AND p_version IS NULL)
    THEN RAISE EXCEPTION 'invalid calendar query' USING ERRCODE='B2204'; END IF;
    IF p_specialist IS NOT NULL AND NOT EXISTS(SELECT 1 FROM bookings.specialists WHERE tenant_id=settings.tenant_id AND location_id=settings.location_id AND id=p_specialist AND enabled)
    THEN RAISE EXCEPTION 'specialist changed' USING ERRCODE='B2205'; END IF;
    WITH candidates AS MATERIALIZED (
        SELECT o.*,p.name AS specialist_name,p.version AS specialist_version,
            CASE WHEN o.starts_at IS NULL THEN o.local_date ELSE (o.starts_at AT TIME ZONE zone)::date END AS public_date
        FROM bookings.offers o JOIN bookings.specialists p ON p.tenant_id=o.tenant_id AND p.id=o.specialist_id
        WHERE o.tenant_id=settings.tenant_id AND o.location_id=settings.location_id AND o.state='ready' AND p.enabled AND (p_specialist IS NULL OR o.specialist_id=p_specialist)
            AND ((o.starts_at>current_timestamp AND o.starts_at >= month_start::timestamp AT TIME ZONE zone AND o.starts_at<(month_start+interval '1 month') AT TIME ZONE zone)
                OR (o.starts_at IS NULL AND (o.local_date IS NULL OR (o.local_date>=(current_timestamp AT TIME ZONE zone)::date AND o.local_date>=month_start AND o.local_date<month_start+interval '1 month'))))
    ), available AS MATERIALIZED (
        SELECT o.*,(o.places-(SELECT count(*) FROM bookings.appointments a WHERE a.tenant_id=o.tenant_id AND a.offer_id=o.id AND a.status='confirmed')) AS remaining
        FROM candidates o WHERE (SELECT count(*) FROM bookings.appointments a WHERE a.tenant_id=o.tenant_id AND a.offer_id=o.id AND a.status='confirmed')<o.places
            AND NOT EXISTS(SELECT 1 FROM bookings.appointments a WHERE a.tenant_id=o.tenant_id AND a.specialist_id=o.specialist_id AND a.offer_id<>o.id AND a.status='confirmed' AND bookings.times_conflict(a.starts_at,a.ends_at,o.starts_at,o.ends_at))
    ), filtered AS MATERIALIZED (
        SELECT * FROM available WHERE public_date=chosen_date OR public_date IS NULL
    ), page AS MATERIALIZED (
        SELECT * FROM filtered o WHERE p_after IS NULL OR (coalesce(o.starts_at,'infinity'::timestamptz),o.id)>(SELECT coalesce(a.starts_at,'infinity'::timestamptz),a.id FROM filtered a WHERE a.id=p_after)
        ORDER BY starts_at NULLS LAST,id LIMIT p_limit+1
    ) SELECT md5(profile::text||p_month||p_date||coalesce(p_specialist::text,'')||coalesce((SELECT string_agg(id::text||':'||version::text||':'||remaining::text,',' ORDER BY id) FROM available),'')),
        (p_after IS NULL OR EXISTS(SELECT 1 FROM filtered WHERE id=p_after)),
        jsonb_build_object('profile',profile,'month',p_month,'date',p_date,
            'days',coalesce((SELECT jsonb_agg(jsonb_build_object('date',to_char(day,'YYYY-MM-DD'),'count',amount) ORDER BY day) FROM (SELECT public_date AS day,count(*) AS amount FROM available WHERE public_date IS NOT NULL GROUP BY public_date) days),'[]'),
            'items',coalesce((SELECT jsonb_agg(jsonb_build_object('id',id,'specialist_id',specialist_id,'specialist_name',specialist_name,'specialist_version',specialist_version,'title',title,'version',version,'mode',mode,
                'date',to_char(public_date,'YYYY-MM-DD'),'starts_local',to_char(starts_at AT TIME ZONE zone,'YYYY-MM-DD"T"HH24:MI'),'ends_local',to_char(ends_at AT TIME ZONE zone,'YYYY-MM-DD"T"HH24:MI'),'time_zone',zone,'starts_at',to_char(starts_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"'),'ends_at',to_char(ends_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"'),'remaining',remaining) ORDER BY starts_at NULLS LAST,id)
                FROM (SELECT * FROM page ORDER BY starts_at NULLS LAST,id LIMIT p_limit) visible),'[]'),
            'next_cursor',CASE WHEN (SELECT count(*) FROM page)>p_limit THEN (SELECT id FROM page ORDER BY starts_at NULLS LAST,id OFFSET p_limit-1 LIMIT 1) ELSE NULL END)
        INTO snapshot,cursor_exists,result;
    IF (p_version IS NOT NULL AND p_version<>snapshot) OR NOT cursor_exists THEN RAISE EXCEPTION 'calendar changed' USING ERRCODE='B2205'; END IF;
    RETURN result||jsonb_build_object('version',snapshot);
END $f$;
REVOKE ALL ON FUNCTION bookings.public_availability(uuid,text,text,uuid,uuid,text,integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION bookings.public_availability(uuid,text,text,uuid,uuid,text,integer) TO turnesta_runtime;

CREATE OR REPLACE FUNCTION bookings.guard_appointment() RETURNS trigger LANGUAGE plpgsql SECURITY INVOKER SET search_path=pg_catalog AS $f$
DECLARE target bookings.offers%ROWTYPE; actor uuid; new_place boolean; public_insert boolean;
BEGIN
    actor:=nullif(current_setting('app.user_id',true),'')::uuid;
    public_insert:=TG_OP='INSERT' AND NEW.source='public' AND current_user=(SELECT pg_get_userbyid(relowner) FROM pg_class WHERE oid='bookings.appointments'::regclass);
    IF NOT public_insert AND (NEW.tenant_id IS DISTINCT FROM nullif(current_setting('app.tenant_id',true),'')::uuid OR NOT EXISTS (
        SELECT 1 FROM core.memberships m WHERE m.tenant_id=NEW.tenant_id AND m.user_id=actor AND m.role IN ('owner','manager','operator')))
    THEN RAISE EXCEPTION 'forbidden' USING ERRCODE='42501'; END IF;
    IF TG_OP='INSERT' AND NEW.source<>'staff' AND NOT public_insert THEN RAISE EXCEPTION 'forbidden source' USING ERRCODE='42501'; END IF;
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
        NEW.created_by:=CASE WHEN public_insert THEN NULL ELSE actor END; NEW.created_at:=clock_timestamp();
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
    NEW.updated_by:=CASE WHEN public_insert THEN NULL ELSE actor END; NEW.updated_at:=clock_timestamp();
    NEW.cancelled_at:=CASE WHEN NEW.status='cancelled' THEN clock_timestamp() ELSE NULL END;
    UPDATE bookings.calendars SET revision=revision+1 WHERE tenant_id=NEW.tenant_id AND location_id=NEW.location_id AND specialist_id=NEW.specialist_id;
    RETURN NEW;
END $f$;

CREATE OR REPLACE VIEW bookings.appointment_details WITH(security_invoker=true) AS
SELECT tenant_id,location_id,specialist_id,offer_id,id,client_name,phone,email,note,status,source,title,specialist_name,mode,
    to_char(local_date,'YYYY-MM-DD') AS date,to_char(starts_local,'YYYY-MM-DD"T"HH24:MI') AS starts_local,
    to_char(ends_local,'YYYY-MM-DD"T"HH24:MI') AS ends_local,
    to_char(starts_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"') AS starts_at,
    to_char(ends_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"') AS ends_at,time_zone,
    created_by,updated_by,to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS created_at,
    to_char(updated_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS updated_at,
    to_char(cancelled_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS cancelled_at,version,form_snapshot,answers
FROM bookings.appointments;
