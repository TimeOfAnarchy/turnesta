CREATE SCHEMA audit AUTHORIZATION CURRENT_USER;
REVOKE ALL ON SCHEMA audit FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;
GRANT USAGE ON SCHEMA audit TO turnesta_runtime;

CREATE TABLE audit.settings (
    tenant_id uuid PRIMARY KEY REFERENCES core.organizations(id),
    retention_days integer NOT NULL DEFAULT 365 CHECK(retention_days BETWEEN 30 AND 3650),
    version integer NOT NULL DEFAULT 1 CHECK(version>0)
);
ALTER TABLE audit.settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit.settings FORCE ROW LEVEL SECURITY;
CREATE POLICY owner_access ON audit.settings TO turnesta_runtime USING (
    tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid AND EXISTS (
        SELECT 1 FROM core.memberships m WHERE m.tenant_id=settings.tenant_id AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.active AND m.role='owner'))
    WITH CHECK (tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid AND EXISTS (
        SELECT 1 FROM core.memberships m WHERE m.tenant_id=settings.tenant_id AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.active AND m.role='owner'));
CREATE POLICY internal_access ON audit.settings TO CURRENT_USER USING(true) WITH CHECK(true);
REVOKE ALL ON audit.settings FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;
GRANT SELECT,INSERT ON audit.settings TO turnesta_runtime;
GRANT UPDATE(retention_days,version) ON audit.settings TO turnesta_runtime;
CREATE FUNCTION audit.guard_settings() RETURNS trigger LANGUAGE plpgsql SECURITY INVOKER SET search_path=pg_catalog AS $f$
BEGIN
    IF (TG_OP='INSERT' AND NEW.version<>1) OR
       (TG_OP='UPDATE' AND (NEW.tenant_id IS DISTINCT FROM OLD.tenant_id OR NEW.version<>OLD.version+1))
    THEN RAISE EXCEPTION 'invalid audit settings version' USING ERRCODE='42501'; END IF;
    RETURN NEW;
END $f$;
REVOKE ALL ON FUNCTION audit.guard_settings() FROM PUBLIC;
CREATE TRIGGER guard_settings BEFORE INSERT OR UPDATE ON audit.settings FOR EACH ROW EXECUTE FUNCTION audit.guard_settings();

CREATE TABLE audit.access_log (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id uuid NOT NULL,
    actor_id uuid NOT NULL,
    location_id uuid NOT NULL,
    action text NOT NULL CHECK(action IN ('list','detail','command')),
    appointment_id uuid,
    specialist_id uuid,
    visit_date date,
    after_id uuid,
    page_limit integer,
    include_cancelled boolean,
    result_count integer,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY(tenant_id,actor_id) REFERENCES core.memberships(tenant_id,user_id),
    FOREIGN KEY(tenant_id,location_id) REFERENCES core.locations(tenant_id,id),
    CHECK (((action='list' AND appointment_id IS NULL AND specialist_id IS NOT NULL AND visit_date IS NOT NULL
            AND page_limit BETWEEN 1 AND 100 AND include_cancelled IS NOT NULL AND result_count BETWEEN 0 AND 101)
        OR (action IN ('detail','command') AND appointment_id IS NOT NULL AND specialist_id IS NULL AND visit_date IS NULL
            AND after_id IS NULL AND page_limit IS NULL AND include_cancelled IS NULL AND result_count IS NULL)) IS TRUE)
);
CREATE INDEX access_log_tenant_time ON audit.access_log(tenant_id,created_at DESC,id DESC);
CREATE INDEX access_log_actor_time ON audit.access_log(tenant_id,actor_id,created_at DESC,id DESC);
ALTER TABLE audit.access_log ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit.access_log FORCE ROW LEVEL SECURITY;
CREATE POLICY internal_access ON audit.access_log TO CURRENT_USER USING(true) WITH CHECK(true);
REVOKE ALL ON audit.access_log FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;
REVOKE ALL ON SEQUENCE audit.access_log_id_seq FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;

CREATE FUNCTION audit.lock_owner(p_tenant uuid) RETURNS void LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE actor uuid:=nullif(current_setting('app.user_id',true),'')::uuid;
BEGIN
    IF p_tenant IS DISTINCT FROM nullif(current_setting('app.tenant_id',true),'')::uuid OR actor IS NULL THEN RAISE EXCEPTION 'invalid audit context' USING ERRCODE='42501'; END IF;
    PERFORM 1 FROM core.memberships m WHERE m.tenant_id=p_tenant AND m.user_id=actor AND m.active AND m.role='owner' FOR SHARE;
    IF NOT FOUND THEN RAISE EXCEPTION 'owner required' USING ERRCODE='42501'; END IF;
END $f$;
REVOKE ALL ON FUNCTION audit.lock_owner(uuid) FROM PUBLIC,turnesta_provisioner,turnesta_delivery_worker;
GRANT EXECUTE ON FUNCTION audit.lock_owner(uuid) TO turnesta_runtime;

CREATE FUNCTION audit.record_access(p_tenant uuid,p_location uuid,p_action text,p_appointment uuid,p_specialist uuid,p_date text,p_after uuid,p_limit integer,p_cancelled boolean,p_count integer)
RETURNS void LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE actor uuid:=nullif(current_setting('app.user_id',true),'')::uuid; visit date;
BEGIN
    IF p_tenant IS DISTINCT FROM nullif(current_setting('app.tenant_id',true),'')::uuid OR actor IS NULL THEN
        RAISE EXCEPTION 'invalid audit context' USING ERRCODE='42501';
    END IF;
    PERFORM 1 FROM core.memberships m WHERE m.tenant_id=p_tenant AND m.user_id=actor AND m.active AND m.role IN ('owner','manager','operator') FOR SHARE;
    IF NOT FOUND OR NOT EXISTS(SELECT 1 FROM core.locations l WHERE l.tenant_id=p_tenant AND l.id=p_location) THEN
        RAISE EXCEPTION 'audit access denied' USING ERRCODE='42501';
    END IF;
    IF p_action IN ('detail','command') AND NOT EXISTS(SELECT 1 FROM bookings.appointments a WHERE a.tenant_id=p_tenant AND a.location_id=p_location AND a.id=p_appointment) THEN
        RAISE EXCEPTION 'appointment unavailable' USING ERRCODE='42501';
    END IF;
    IF p_date IS NOT NULL THEN visit:=bookings.valid_date(p_date); END IF;
    INSERT INTO audit.settings(tenant_id) VALUES(p_tenant) ON CONFLICT DO NOTHING;
    INSERT INTO audit.access_log(tenant_id,actor_id,location_id,action,appointment_id,specialist_id,visit_date,after_id,page_limit,include_cancelled,result_count)
        VALUES(p_tenant,actor,p_location,p_action,p_appointment,p_specialist,visit,p_after,p_limit,p_cancelled,p_count);
END $f$;
REVOKE ALL ON FUNCTION audit.record_access(uuid,uuid,text,uuid,uuid,text,uuid,integer,boolean,integer) FROM PUBLIC,turnesta_provisioner,turnesta_delivery_worker;
GRANT EXECUTE ON FUNCTION audit.record_access(uuid,uuid,text,uuid,uuid,text,uuid,integer,boolean,integer) TO turnesta_runtime;

CREATE FUNCTION audit.list_access(p_tenant uuid,p_from text,p_to text,p_actor uuid,p_before_at text,p_before_id bigint,p_limit integer)
RETURNS TABLE(id text,actor_id uuid,actor_name text,location_id uuid,location_name text,action text,appointment_id uuid,specialist_id uuid,visit_date text,after_id uuid,page_limit integer,include_cancelled boolean,result_count integer,created_at text)
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE actor uuid:=nullif(current_setting('app.user_id',true),'')::uuid; from_day date; to_day date; before_time timestamptz;
BEGIN
    IF p_tenant IS DISTINCT FROM nullif(current_setting('app.tenant_id',true),'')::uuid OR actor IS NULL OR p_limit IS NULL OR p_limit NOT BETWEEN 1 AND 100 OR (p_before_at IS NULL)<>(p_before_id IS NULL) OR p_before_id<=0 THEN
        RAISE EXCEPTION 'invalid audit query' USING ERRCODE='42501';
    END IF;
    PERFORM 1 FROM core.memberships m WHERE m.tenant_id=p_tenant AND m.user_id=actor AND m.active AND m.role='owner' FOR SHARE;
    IF NOT FOUND THEN RAISE EXCEPTION 'owner required' USING ERRCODE='42501'; END IF;
    IF p_from IS NOT NULL THEN from_day:=bookings.valid_date(p_from); END IF;
    IF p_to IS NOT NULL THEN to_day:=bookings.valid_date(p_to); END IF;
    IF from_day>to_day THEN RAISE EXCEPTION 'invalid audit date range' USING ERRCODE='B2001'; END IF;
    IF p_before_at IS NOT NULL THEN before_time:=p_before_at::timestamptz; END IF;
    RETURN QUERY SELECT a.id::text,a.actor_id,u.display_name,a.location_id,l.name,a.action,a.appointment_id,a.specialist_id,
        to_char(a.visit_date,'YYYY-MM-DD'),a.after_id,a.page_limit,a.include_cancelled,a.result_count,
        to_char(a.created_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
    FROM audit.access_log a JOIN core.users u ON u.id=a.actor_id JOIN core.locations l ON l.tenant_id=a.tenant_id AND l.id=a.location_id
    WHERE a.tenant_id=p_tenant AND (from_day IS NULL OR a.created_at>=from_day::timestamp AT TIME ZONE 'UTC')
        AND (to_day IS NULL OR a.created_at<(to_day+1)::timestamp AT TIME ZONE 'UTC')
        AND (p_actor IS NULL OR a.actor_id=p_actor)
        AND (before_time IS NULL OR (a.created_at,a.id)<(before_time,p_before_id))
    ORDER BY a.created_at DESC,a.id DESC LIMIT p_limit+1;
END $f$;
REVOKE ALL ON FUNCTION audit.list_access(uuid,text,text,uuid,text,bigint,integer) FROM PUBLIC,turnesta_provisioner,turnesta_delivery_worker;
GRANT EXECUTE ON FUNCTION audit.list_access(uuid,text,text,uuid,text,bigint,integer) TO turnesta_runtime;

CREATE FUNCTION audit.purge_batch(p_dry_run boolean) RETURNS bigint
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE item record; days integer; count_rows bigint:=0; deleted bigint; cutoff timestamptz:=clock_timestamp();
BEGIN
    IF session_user<>'turnesta_migrator' OR p_dry_run IS NULL THEN RAISE EXCEPTION 'maintenance login required' USING ERRCODE='42501'; END IF;
    IF p_dry_run THEN
        SELECT count(*) INTO count_rows FROM audit.access_log a JOIN audit.settings s ON s.tenant_id=a.tenant_id
            WHERE a.created_at < cutoff-make_interval(days=>s.retention_days);
        RETURN count_rows;
    END IF;
    FOR item IN SELECT tenant_id FROM audit.settings ORDER BY tenant_id LOOP
        SELECT retention_days INTO days FROM audit.settings WHERE tenant_id=item.tenant_id FOR SHARE SKIP LOCKED;
        IF NOT FOUND THEN CONTINUE; END IF;
        WITH victims AS (SELECT id FROM audit.access_log WHERE tenant_id=item.tenant_id AND created_at<cutoff-make_interval(days=>days)
            ORDER BY created_at,id LIMIT (1000-count_rows) FOR UPDATE SKIP LOCKED),
        removed AS (DELETE FROM audit.access_log a USING victims v WHERE a.id=v.id RETURNING 1)
        SELECT count(*) INTO deleted FROM removed;
        count_rows:=count_rows+deleted;
        EXIT WHEN count_rows>=1000;
    END LOOP;
    RETURN count_rows;
END $f$;
REVOKE ALL ON FUNCTION audit.purge_batch(boolean) FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;
