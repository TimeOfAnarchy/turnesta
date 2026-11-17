ALTER TABLE core.organizations ADD COLUMN booking_retention_months integer
    CHECK (booking_retention_months BETWEEN 1 AND 1200);
GRANT UPDATE(booking_retention_months) ON core.organizations TO turnesta_runtime;

-- The trusted maintenance login may inspect retention settings across tenants.
CREATE POLICY retention_owner_read ON core.organizations FOR SELECT TO CURRENT_USER USING (true);

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


-- Both the user command and the maintenance path use the same narrow erasure.
CREATE FUNCTION bookings.erase_appointment_fields(p_tenant uuid,p_location uuid,p_appointment uuid,p_expected_version integer)
RETURNS void LANGUAGE plpgsql SECURITY INVOKER SET search_path=pg_catalog AS $f$
BEGIN
    UPDATE bookings.appointments SET client_name='Удалено',phone='',email='',note='',answers='{}'::jsonb,anonymized_at=clock_timestamp(),version=version+1
    WHERE tenant_id=p_tenant AND location_id=p_location AND id=p_appointment AND version=p_expected_version AND anonymized_at IS NULL;
    IF NOT FOUND THEN RAISE EXCEPTION 'appointment changed' USING ERRCODE='B2107'; END IF;
    UPDATE bookings.public_claims SET request_fingerprint=decode(repeat('00',32),'hex')
        WHERE tenant_id=p_tenant AND location_id=p_location AND appointment_id=p_appointment;
END $f$;
REVOKE ALL ON FUNCTION bookings.erase_appointment_fields(uuid,uuid,uuid,integer) FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;

CREATE OR REPLACE FUNCTION bookings.anonymize_appointment(p_tenant uuid,p_location uuid,p_appointment uuid,p_actor uuid,p_expected_version integer)
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
    PERFORM bookings.erase_appointment_fields(p_tenant,p_location,p_appointment,p_expected_version);
END $f$;

CREATE FUNCTION bookings.retention_batch(p_dry_run boolean) RETURNS bigint
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE candidate record; settings record; current_row record; result bigint:=0; cutoff timestamptz:=clock_timestamp();
BEGIN
    IF session_user<>'turnesta_migrator' OR p_dry_run IS NULL THEN RAISE EXCEPTION 'maintenance login required' USING ERRCODE='42501'; END IF;
    IF p_dry_run THEN
        SELECT count(*) INTO result FROM bookings.appointments a JOIN core.organizations o ON o.id=a.tenant_id
        WHERE o.booking_retention_months IS NOT NULL AND a.anonymized_at IS NULL AND
            (CASE WHEN a.status='cancelled' THEN a.cancelled_at ELSE a.ends_at END) < cutoff-make_interval(months=>o.booking_retention_months);
        RETURN result;
    END IF;
    FOR candidate IN SELECT a.tenant_id,a.location_id,a.specialist_id,a.id FROM bookings.appointments a JOIN core.organizations o ON o.id=a.tenant_id
        WHERE o.booking_retention_months IS NOT NULL AND a.anonymized_at IS NULL AND
            (CASE WHEN a.status='cancelled' THEN a.cancelled_at ELSE a.ends_at END) < cutoff-make_interval(months=>o.booking_retention_months)
        ORDER BY a.tenant_id,a.location_id,a.specialist_id,a.id LIMIT 1000
    LOOP
        SELECT booking_retention_months INTO settings FROM core.organizations WHERE id=candidate.tenant_id FOR SHARE;
        IF NOT FOUND OR settings.booking_retention_months IS NULL THEN CONTINUE; END IF;
        PERFORM set_config('app.tenant_id',candidate.tenant_id::text,true),set_config('app.user_id','',true);
        PERFORM 1 FROM bookings.calendars WHERE tenant_id=candidate.tenant_id AND location_id=candidate.location_id AND specialist_id=candidate.specialist_id FOR UPDATE SKIP LOCKED;
        IF NOT FOUND THEN CONTINUE; END IF;
        SELECT version INTO current_row FROM bookings.appointments a WHERE a.tenant_id=candidate.tenant_id AND a.location_id=candidate.location_id AND a.id=candidate.id AND a.anonymized_at IS NULL AND
            (CASE WHEN a.status='cancelled' THEN a.cancelled_at ELSE a.ends_at END) < cutoff-make_interval(months=>settings.booking_retention_months);
        IF NOT FOUND THEN CONTINUE; END IF;
        PERFORM bookings.erase_appointment_fields(candidate.tenant_id,candidate.location_id,candidate.id,current_row.version);
        result:=result+1;
    END LOOP;
    RETURN result;
END $f$;
REVOKE ALL ON FUNCTION bookings.retention_batch(boolean) FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;
