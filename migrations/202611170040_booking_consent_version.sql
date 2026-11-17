-- Existing records keep NULL: their exact displayed text was not recorded.
ALTER TABLE bookings.public_settings ADD COLUMN consent_text text NOT NULL DEFAULT 'Согласен(на) на обработку персональных данных в соответствии с политикой организации';
ALTER TABLE bookings.public_settings ADD COLUMN consent_version integer NOT NULL DEFAULT 1 CHECK(consent_version>0);
ALTER TABLE bookings.public_settings ADD CONSTRAINT public_consent_text_valid CHECK (
    char_length(consent_text) BETWEEN 1 AND 1000 AND octet_length(consent_text)<=4000
    AND consent_text=btrim(consent_text) AND consent_text !~ '[[:cntrl:]]'
);
GRANT UPDATE(consent_text) ON bookings.public_settings TO turnesta_runtime;
CREATE FUNCTION bookings.version_consent() RETURNS trigger LANGUAGE plpgsql SECURITY INVOKER SET search_path=pg_catalog AS $f$
BEGIN
    IF TG_OP='INSERT' THEN NEW.consent_version:=1;
    ELSIF NEW.consent_text IS DISTINCT FROM OLD.consent_text THEN NEW.consent_version:=OLD.consent_version+1;
    ELSE NEW.consent_version:=OLD.consent_version; END IF;
    RETURN NEW;
END $f$;
REVOKE ALL ON FUNCTION bookings.version_consent() FROM PUBLIC;
CREATE TRIGGER version_consent BEFORE INSERT OR UPDATE ON bookings.public_settings FOR EACH ROW EXECUTE FUNCTION bookings.version_consent();

ALTER TABLE bookings.appointments ADD COLUMN consent_version integer;
ALTER TABLE bookings.appointments ADD COLUMN consent_text_sha256 bytea;
ALTER TABLE bookings.appointments ADD CONSTRAINT appointment_consent_snapshot CHECK (
    (consent_version IS NULL AND consent_text_sha256 IS NULL)
    OR (consent_version IS NOT NULL AND consent_version>0 AND consent_text_sha256 IS NOT NULL AND octet_length(consent_text_sha256)=32 AND consent_at IS NOT NULL AND source='public')
);

CREATE OR REPLACE FUNCTION bookings.guard_appointment() RETURNS trigger LANGUAGE plpgsql SECURITY INVOKER SET search_path=pg_catalog AS $f$
DECLARE target bookings.offers%ROWTYPE; actor uuid; new_place boolean; public_insert boolean; maintenance boolean; erasing boolean:=false;
BEGIN
    actor:=nullif(current_setting('app.user_id',true),'')::uuid;
    maintenance:=session_user='turnesta_migrator' AND current_user=(SELECT pg_get_userbyid(relowner) FROM pg_class WHERE oid='bookings.appointments'::regclass) AND actor IS NULL AND TG_OP='UPDATE' AND OLD.anonymized_at IS NULL AND NEW.anonymized_at IS NOT NULL;
    public_insert:=TG_OP='INSERT' AND NEW.source='public' AND current_user=(SELECT pg_get_userbyid(relowner) FROM pg_class WHERE oid='bookings.appointments'::regclass);
    IF NOT public_insert AND NOT maintenance AND (NEW.tenant_id IS DISTINCT FROM nullif(current_setting('app.tenant_id',true),'')::uuid OR NOT EXISTS (
        SELECT 1 FROM core.memberships m WHERE m.tenant_id=NEW.tenant_id AND m.user_id=actor AND m.active AND m.role IN ('owner','manager','operator')))
    THEN RAISE EXCEPTION 'forbidden' USING ERRCODE='42501'; END IF;
    IF TG_OP='INSERT' AND (NEW.anonymized_at IS NOT NULL OR (NEW.source<>'staff' AND NOT public_insert)) THEN RAISE EXCEPTION 'forbidden source or anonymized insert' USING ERRCODE='42501'; END IF;
    PERFORM 1 FROM bookings.calendars WHERE tenant_id=NEW.tenant_id AND location_id=NEW.location_id AND specialist_id=NEW.specialist_id FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'calendar not found' USING ERRCODE='B2101'; END IF;
    IF TG_OP='UPDATE' THEN
        IF (NEW.consent_url,NEW.consent_at,NEW.consent_version,NEW.consent_text_sha256) IS DISTINCT FROM (OLD.consent_url,OLD.consent_at,OLD.consent_version,OLD.consent_text_sha256)
        THEN RAISE EXCEPTION 'consent snapshot is immutable' USING ERRCODE='B2102'; END IF;
        erasing:=OLD.anonymized_at IS NULL AND NEW.anonymized_at IS NOT NULL;
        IF erasing AND (current_user<>(SELECT pg_get_userbyid(relowner) FROM pg_class WHERE oid='bookings.appointments'::regclass)
            OR (NOT maintenance AND NOT EXISTS(SELECT 1 FROM core.memberships m WHERE m.tenant_id=NEW.tenant_id AND m.user_id=actor AND m.active AND m.role IN ('owner','manager')))
            OR (to_jsonb(NEW)-ARRAY['client_name','phone','email','note','answers','anonymized_at','version']) IS DISTINCT FROM (to_jsonb(OLD)-ARRAY['client_name','phone','email','note','answers','anonymized_at','version']))
        THEN RAISE EXCEPTION 'invalid anonymization' USING ERRCODE='42501'; END IF;
        IF OLD.anonymized_at IS NOT NULL AND (NEW.anonymized_at,NEW.client_name,NEW.phone,NEW.email,NEW.note,NEW.answers) IS DISTINCT FROM (OLD.anonymized_at,OLD.client_name,OLD.phone,OLD.email,OLD.note,OLD.answers)
        THEN RAISE EXCEPTION 'anonymized data cannot be restored' USING ERRCODE='B2102'; END IF;
        IF (NEW.tenant_id,NEW.location_id,NEW.specialist_id,NEW.id) IS DISTINCT FROM (OLD.tenant_id,OLD.location_id,OLD.specialist_id,OLD.id)
            OR NEW.version<>OLD.version+1 OR (OLD.status='cancelled' AND NOT erasing)
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
    NEW.cancelled_at:=CASE WHEN erasing THEN OLD.cancelled_at WHEN NEW.status='cancelled' THEN clock_timestamp() ELSE NULL END;
    UPDATE bookings.calendars SET revision=revision+1 WHERE tenant_id=NEW.tenant_id AND location_id=NEW.location_id AND specialist_id=NEW.specialist_id;
    RETURN NEW;
END $f$;


CREATE OR REPLACE FUNCTION bookings.public_profile(p_slug uuid) RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $f$
SELECT jsonb_build_object('public_slug',s.public_slug,'organization_name',o.name,'location_name',l.name,'time_zone',l.time_zone,
    'today',to_char(current_timestamp AT TIME ZONE l.time_zone,'YYYY-MM-DD'),'enabled',s.enabled,'version',s.version,
    'privacy_url',CASE WHEN s.enabled THEN s.privacy_url ELSE NULL END,
    'consent_text',CASE WHEN s.enabled THEN s.consent_text ELSE NULL END,
    'consent_version',CASE WHEN s.enabled THEN s.consent_version ELSE NULL END,
    'form',CASE WHEN s.enabled THEN s.form ELSE NULL END,'specialists',CASE WHEN s.enabled THEN coalesce((SELECT jsonb_agg(jsonb_build_object('id',p.id,'name',p.name,'description',p.description,'version',p.version) ORDER BY p.name,p.id)
        FROM bookings.specialists p WHERE p.tenant_id=s.tenant_id AND p.location_id=s.location_id AND p.enabled),'[]') ELSE '[]'::jsonb END)
FROM bookings.public_settings s JOIN core.locations l ON l.tenant_id=s.tenant_id AND l.id=s.location_id JOIN core.organizations o ON o.id=s.tenant_id WHERE s.public_slug=p_slug
$f$;

CREATE FUNCTION bookings.public_book(p_slug uuid,p_hash bytea,p_offer uuid,p_offer_version integer,p_specialist_version integer,p_form_version integer,p_zone text,p_answers jsonb,p_consent boolean,p_consent_version integer)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE settings bookings.public_settings%ROWTYPE; target bookings.offers%ROWTYPE; fingerprint bytea; normalized jsonb; previous bytea; erased boolean; appointment uuid:=gen_random_uuid(); point_zone text;
BEGIN
    IF octet_length(p_hash) IS DISTINCT FROM 32 THEN RAISE EXCEPTION 'invalid access' USING ERRCODE='B2204'; END IF;
    normalized:=bookings.clean_answers(p_answers);
    IF p_offer IS NULL OR p_offer_version IS NULL OR p_offer_version<1 OR p_specialist_version IS NULL OR p_specialist_version<1 OR p_form_version IS NULL OR p_form_version<1 OR p_zone IS NULL THEN RAISE EXCEPTION 'invalid booking' USING ERRCODE='B2204'; END IF;
    fingerprint:=sha256(convert_to(jsonb_build_object('offer_id',p_offer,'offer_version',p_offer_version,'specialist_version',p_specialist_version,'form_version',p_form_version,'time_zone',p_zone,'answers',normalized)::text,'UTF8'));
    SELECT * INTO settings FROM bookings.public_settings WHERE public_slug=p_slug FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'booking page not found' USING ERRCODE='B2201'; END IF;
    IF EXISTS(SELECT 1 FROM retention.used_keys t WHERE t.kind='bookings.public_claims' AND t.tenant_id=settings.tenant_id AND t.scope_id=settings.location_id AND t.actor_id='' AND t.key_value=to_jsonb(p_hash)#>>'{}') THEN RAISE EXCEPTION 'receipt expired' USING ERRCODE='B2207'; END IF;
    SELECT c.request_fingerprint, a.anonymized_at IS NOT NULL INTO previous, erased FROM bookings.public_claims c JOIN bookings.appointments a ON a.tenant_id=c.tenant_id AND a.location_id=c.location_id AND a.id=c.appointment_id WHERE c.tenant_id=settings.tenant_id AND c.location_id=settings.location_id AND c.token_hash=p_hash;
    IF FOUND THEN
        IF NOT erased AND previous<>fingerprint THEN RAISE EXCEPTION 'different request' USING ERRCODE='B2206'; END IF;
        RETURN bookings.public_receipt(p_slug,p_hash);
    END IF;
    IF NOT settings.enabled THEN RAISE EXCEPTION 'booking paused' USING ERRCODE='B2202'; END IF;
    SELECT time_zone INTO point_zone FROM core.locations WHERE tenant_id=settings.tenant_id AND id=settings.location_id;
    IF settings.version<>p_form_version OR point_zone<>p_zone THEN RAISE EXCEPTION 'review changed form or timezone' USING ERRCODE='B2205'; END IF;
    IF p_consent_version IS DISTINCT FROM settings.consent_version THEN RAISE EXCEPTION 'review changed consent' USING ERRCODE='B2205'; END IF;
    IF p_consent IS NOT TRUE THEN RAISE EXCEPTION 'consent required' USING ERRCODE='B2204'; END IF;
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
        OR (SELECT count(*) FROM bookings.appointments a WHERE a.tenant_id=settings.tenant_id AND a.location_id=settings.location_id AND a.source='public' AND a.status='confirmed' AND (a.starts_at>clock_timestamp() OR (a.starts_at IS NULL AND (a.local_date IS NULL OR a.local_date>=(clock_timestamp() AT TIME ZONE point_zone)::date))))>=1000
    THEN RAISE EXCEPTION 'booking rate exceeded' USING ERRCODE='B2203'; END IF;
    IF coalesce(normalized->>'phone','') <> '' AND (SELECT count(*) FROM bookings.appointments a WHERE a.tenant_id=settings.tenant_id AND a.location_id=settings.location_id AND a.source='public' AND a.status='confirmed' AND regexp_replace(a.phone,'[^0-9]','','g')=regexp_replace(normalized->>'phone','[^0-9]','','g') AND (a.starts_at>clock_timestamp() OR (a.starts_at IS NULL AND (a.local_date IS NULL OR a.local_date>=(clock_timestamp() AT TIME ZONE point_zone)::date))))>=3 THEN
        RAISE EXCEPTION 'phone booking limit exceeded' USING ERRCODE='B2208';
    END IF;
    INSERT INTO bookings.appointments(tenant_id,location_id,specialist_id,offer_id,id,client_name,phone,email,note,status,source,form_snapshot,answers,consent_url,consent_at,consent_version,consent_text_sha256)
        VALUES(target.tenant_id,target.location_id,target.specialist_id,target.id,appointment,normalized->>'client_name',coalesce(normalized->>'phone',''),coalesce(normalized->>'email',''),coalesce(normalized->>'note',''),'confirmed','public',settings.form,normalized,settings.privacy_url,clock_timestamp(),settings.consent_version,sha256(convert_to(settings.consent_text,'UTF8')));
    INSERT INTO bookings.public_claims(tenant_id,location_id,token_hash,appointment_id,request_fingerprint,public_zone)
        VALUES(target.tenant_id,target.location_id,p_hash,appointment,fingerprint,point_zone);
    RETURN bookings.public_receipt(p_slug,p_hash);
END $f$;
REVOKE ALL ON FUNCTION bookings.public_book(uuid,bytea,uuid,integer,integer,integer,text,jsonb,boolean) FROM PUBLIC,turnesta_runtime;
REVOKE ALL ON FUNCTION bookings.public_book(uuid,bytea,uuid,integer,integer,integer,text,jsonb,boolean,integer) FROM PUBLIC,turnesta_provisioner,turnesta_delivery_worker;
GRANT EXECUTE ON FUNCTION bookings.public_book(uuid,bytea,uuid,integer,integer,integer,text,jsonb,boolean,integer) TO turnesta_runtime;
