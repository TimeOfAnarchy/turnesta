ALTER TABLE bookings.appointments ADD COLUMN anonymized_at timestamptz;
ALTER TABLE bookings.appointments ADD CONSTRAINT appointments_anonymized_fields CHECK (
    anonymized_at IS NULL OR (client_name='Удалено' AND phone='' AND email='' AND note='' AND answers='{}'::jsonb)
);

CREATE OR REPLACE FUNCTION bookings.guard_appointment() RETURNS trigger LANGUAGE plpgsql SECURITY INVOKER SET search_path=pg_catalog AS $f$
DECLARE target bookings.offers%ROWTYPE; actor uuid; new_place boolean; public_insert boolean; erasing boolean:=false;
BEGIN
    actor:=nullif(current_setting('app.user_id',true),'')::uuid;
    public_insert:=TG_OP='INSERT' AND NEW.source='public' AND current_user=(SELECT pg_get_userbyid(relowner) FROM pg_class WHERE oid='bookings.appointments'::regclass);
    IF NOT public_insert AND (NEW.tenant_id IS DISTINCT FROM nullif(current_setting('app.tenant_id',true),'')::uuid OR NOT EXISTS (
        SELECT 1 FROM core.memberships m WHERE m.tenant_id=NEW.tenant_id AND m.user_id=actor AND m.active AND m.role IN ('owner','manager','operator')))
    THEN RAISE EXCEPTION 'forbidden' USING ERRCODE='42501'; END IF;
    IF TG_OP='INSERT' AND (NEW.anonymized_at IS NOT NULL OR (NEW.source<>'staff' AND NOT public_insert)) THEN RAISE EXCEPTION 'forbidden source or anonymized insert' USING ERRCODE='42501'; END IF;
    PERFORM 1 FROM bookings.calendars WHERE tenant_id=NEW.tenant_id AND location_id=NEW.location_id AND specialist_id=NEW.specialist_id FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'calendar not found' USING ERRCODE='B2101'; END IF;
    IF TG_OP='UPDATE' THEN
        erasing:=OLD.anonymized_at IS NULL AND NEW.anonymized_at IS NOT NULL;
        IF erasing AND (current_user<>(SELECT pg_get_userbyid(relowner) FROM pg_class WHERE oid='bookings.appointments'::regclass)
            OR NOT EXISTS(SELECT 1 FROM core.memberships m WHERE m.tenant_id=NEW.tenant_id AND m.user_id=actor AND m.active AND m.role IN ('owner','manager'))
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

CREATE FUNCTION bookings.anonymize_appointment(p_tenant uuid,p_location uuid,p_appointment uuid,p_actor uuid,p_expected_version integer)
RETURNS void LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE specialist uuid;
BEGIN
    IF p_tenant IS DISTINCT FROM nullif(current_setting('app.tenant_id',true),'')::uuid
        OR p_actor IS DISTINCT FROM nullif(current_setting('app.user_id',true),'')::uuid
        OR p_expected_version IS NULL OR p_expected_version<1
    THEN RAISE EXCEPTION 'invalid anonymization context' USING ERRCODE='42501'; END IF;
    PERFORM 1 FROM core.memberships m WHERE m.tenant_id=p_tenant AND m.user_id=p_actor AND m.active AND m.role IN ('owner','manager') FOR SHARE;
    IF NOT FOUND THEN RAISE EXCEPTION 'manager required' USING ERRCODE='42501'; END IF;
    SELECT a.specialist_id INTO specialist FROM bookings.appointments a WHERE a.tenant_id=p_tenant AND a.location_id=p_location AND a.id=p_appointment;
    IF NOT FOUND THEN RAISE EXCEPTION 'appointment not found' USING ERRCODE='B2101'; END IF;
    PERFORM 1 FROM bookings.calendars WHERE tenant_id=p_tenant AND location_id=p_location AND specialist_id=specialist FOR UPDATE;
    UPDATE bookings.appointments SET client_name='Удалено',phone='',email='',note='',answers='{}'::jsonb,anonymized_at=clock_timestamp(),version=version+1
    WHERE tenant_id=p_tenant AND location_id=p_location AND id=p_appointment AND version=p_expected_version AND anonymized_at IS NULL;
    IF NOT FOUND THEN RAISE EXCEPTION 'appointment changed' USING ERRCODE='B2107'; END IF;
    UPDATE bookings.public_claims SET request_fingerprint=decode(repeat('00',32),'hex')
        WHERE tenant_id=p_tenant AND location_id=p_location AND appointment_id=p_appointment;
END $f$;
REVOKE ALL ON FUNCTION bookings.anonymize_appointment(uuid,uuid,uuid,uuid,integer) FROM PUBLIC,turnesta_provisioner,turnesta_delivery_worker;
GRANT EXECUTE ON FUNCTION bookings.anonymize_appointment(uuid,uuid,uuid,uuid,integer) TO turnesta_runtime;

CREATE OR REPLACE VIEW bookings.appointment_details WITH(security_invoker=true) AS
SELECT tenant_id,location_id,specialist_id,offer_id,id,client_name,phone,email,note,status,source,title,specialist_name,mode,
    to_char(local_date,'YYYY-MM-DD') AS date,to_char(starts_local,'YYYY-MM-DD"T"HH24:MI') AS starts_local,
    to_char(ends_local,'YYYY-MM-DD"T"HH24:MI') AS ends_local,
    to_char(starts_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"') AS starts_at,
    to_char(ends_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"') AS ends_at,time_zone,
    created_by,updated_by,to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS created_at,
    to_char(updated_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS updated_at,
    to_char(cancelled_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS cancelled_at,version,form_snapshot,answers,
    to_char(anonymized_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS anonymized_at
FROM bookings.appointments;

-- Erased claims retain only their capability and non-personal receipt.
CREATE OR REPLACE FUNCTION bookings.public_book(p_slug uuid,p_hash bytea,p_offer uuid,p_offer_version integer,p_specialist_version integer,p_form_version integer,p_zone text,p_answers jsonb,p_consent boolean)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE settings bookings.public_settings%ROWTYPE; target bookings.offers%ROWTYPE; fingerprint bytea; normalized jsonb; previous bytea; erased boolean; appointment uuid:=gen_random_uuid(); point_zone text;
BEGIN
    IF octet_length(p_hash) IS DISTINCT FROM 32 THEN RAISE EXCEPTION 'invalid access' USING ERRCODE='B2204'; END IF;
    normalized:=bookings.clean_answers(p_answers);
    IF p_offer IS NULL OR p_offer_version IS NULL OR p_offer_version<1 OR p_specialist_version IS NULL OR p_specialist_version<1 OR p_form_version IS NULL OR p_form_version<1 OR p_zone IS NULL THEN RAISE EXCEPTION 'invalid booking' USING ERRCODE='B2204'; END IF;
    fingerprint:=sha256(convert_to(jsonb_build_object('offer_id',p_offer,'offer_version',p_offer_version,'specialist_version',p_specialist_version,'form_version',p_form_version,'time_zone',p_zone,'answers',normalized)::text,'UTF8'));
    SELECT * INTO settings FROM bookings.public_settings WHERE public_slug=p_slug FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'booking page not found' USING ERRCODE='B2201'; END IF;
    SELECT c.request_fingerprint, a.anonymized_at IS NOT NULL INTO previous, erased FROM bookings.public_claims c JOIN bookings.appointments a ON a.tenant_id=c.tenant_id AND a.location_id=c.location_id AND a.id=c.appointment_id WHERE c.tenant_id=settings.tenant_id AND c.location_id=settings.location_id AND c.token_hash=p_hash;
    IF FOUND THEN
        IF NOT erased AND previous<>fingerprint THEN RAISE EXCEPTION 'different request' USING ERRCODE='B2206'; END IF;
        RETURN bookings.public_receipt(p_slug,p_hash);
    END IF;
    IF NOT settings.enabled THEN RAISE EXCEPTION 'booking paused' USING ERRCODE='B2202'; END IF;
    SELECT time_zone INTO point_zone FROM core.locations WHERE tenant_id=settings.tenant_id AND id=settings.location_id;
    IF settings.version<>p_form_version OR point_zone<>p_zone THEN RAISE EXCEPTION 'review changed form or timezone' USING ERRCODE='B2205'; END IF;
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
        OR (SELECT count(*) FROM bookings.public_claims c JOIN bookings.appointments a ON a.tenant_id=c.tenant_id AND a.id=c.appointment_id WHERE c.tenant_id=settings.tenant_id AND c.location_id=settings.location_id AND a.status='confirmed' AND (a.starts_at>clock_timestamp() OR (a.starts_at IS NULL AND (a.local_date IS NULL OR a.local_date>=(clock_timestamp() AT TIME ZONE point_zone)::date))))>=1000
    THEN RAISE EXCEPTION 'booking rate exceeded' USING ERRCODE='B2203'; END IF;
    IF coalesce(normalized->>'phone','') <> '' AND (SELECT count(*) FROM bookings.appointments a WHERE a.tenant_id=settings.tenant_id AND a.location_id=settings.location_id AND a.source='public' AND a.status='confirmed' AND regexp_replace(a.phone,'[^0-9]','','g')=regexp_replace(normalized->>'phone','[^0-9]','','g') AND (a.starts_at>clock_timestamp() OR (a.starts_at IS NULL AND (a.local_date IS NULL OR a.local_date>=(clock_timestamp() AT TIME ZONE point_zone)::date))))>=3 THEN
        RAISE EXCEPTION 'phone booking limit exceeded' USING ERRCODE='B2208';
    END IF;
    INSERT INTO bookings.appointments(tenant_id,location_id,specialist_id,offer_id,id,client_name,phone,email,note,status,source,form_snapshot,answers,consent_url,consent_at)
        VALUES(target.tenant_id,target.location_id,target.specialist_id,target.id,appointment,normalized->>'client_name',coalesce(normalized->>'phone',''),coalesce(normalized->>'email',''),coalesce(normalized->>'note',''),'confirmed','public',settings.form,normalized,settings.privacy_url,clock_timestamp());
    INSERT INTO bookings.public_claims(tenant_id,location_id,token_hash,appointment_id,request_fingerprint,public_zone)
        VALUES(target.tenant_id,target.location_id,p_hash,appointment,fingerprint,point_zone);
    RETURN bookings.public_receipt(p_slug,p_hash);
END $f$;
REVOKE ALL ON FUNCTION bookings.public_book(uuid,bytea,uuid,integer,integer,integer,text,jsonb,boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION bookings.public_book(uuid,bytea,uuid,integer,integer,integer,text,jsonb,boolean) TO turnesta_runtime;
