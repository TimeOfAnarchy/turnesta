-- Keep extension digits distinct from the main number. Do not infer a country code.
CREATE FUNCTION bookings.export_phone_key(text) RETURNS text
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE SET search_path=pg_catalog AS $$
    SELECT nullif(replace(translate(lower($1),' +().-',''),'#','x'),'')
$$;
CREATE INDEX appointments_export_phone ON bookings.appointments(tenant_id,bookings.export_phone_key(phone))
    WHERE anonymized_at IS NULL AND phone<>'';
CREATE INDEX appointments_export_email ON bookings.appointments(tenant_id,lower(btrim(email)))
    WHERE anonymized_at IS NULL AND email<>'';

ALTER TABLE audit.access_log DROP CONSTRAINT access_log_action_check, DROP CONSTRAINT access_log_check;
ALTER TABLE audit.access_log ADD CONSTRAINT access_log_action_check
    CHECK(action IN ('list','detail','command','export_json','export_csv'));
ALTER TABLE audit.access_log ADD CONSTRAINT access_log_check CHECK ((
    (action='list' AND appointment_id IS NULL AND specialist_id IS NOT NULL AND visit_date IS NOT NULL
        AND page_limit BETWEEN 1 AND 100 AND include_cancelled IS NOT NULL AND result_count BETWEEN 0 AND 101)
    OR (action IN ('detail','command','export_json','export_csv') AND appointment_id IS NOT NULL
        AND specialist_id IS NULL AND visit_date IS NULL AND after_id IS NULL AND page_limit IS NULL
        AND include_cancelled IS NULL AND (CASE WHEN action IN ('export_json','export_csv')
            THEN result_count>0 ELSE result_count IS NULL END))
) IS TRUE);

CREATE OR REPLACE FUNCTION audit.record_access(p_tenant uuid,p_location uuid,p_action text,p_appointment uuid,p_specialist uuid,p_date text,p_after uuid,p_limit integer,p_cancelled boolean,p_count integer)
RETURNS void LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE actor uuid:=nullif(current_setting('app.user_id',true),'')::uuid; visit date;
BEGIN
    IF p_tenant IS DISTINCT FROM nullif(current_setting('app.tenant_id',true),'')::uuid OR actor IS NULL THEN
        RAISE EXCEPTION 'invalid audit context' USING ERRCODE='42501';
    END IF;
    PERFORM 1 FROM core.memberships m WHERE m.tenant_id=p_tenant AND m.user_id=actor AND m.active AND m.role IN ('owner','manager','operator')
        AND (p_action NOT IN ('export_json','export_csv') OR m.role='owner') FOR SHARE;
    IF NOT FOUND OR NOT EXISTS(SELECT 1 FROM core.locations l WHERE l.tenant_id=p_tenant AND l.id=p_location) THEN
        RAISE EXCEPTION 'audit access denied' USING ERRCODE='42501';
    END IF;
    IF p_action IN ('detail','command','export_json','export_csv') AND NOT EXISTS(SELECT 1 FROM bookings.appointments a WHERE a.tenant_id=p_tenant AND a.location_id=p_location AND a.id=p_appointment) THEN
        RAISE EXCEPTION 'appointment unavailable' USING ERRCODE='42501';
    END IF;
    IF p_date IS NOT NULL THEN visit:=bookings.valid_date(p_date); END IF;
    INSERT INTO audit.settings(tenant_id) VALUES(p_tenant) ON CONFLICT DO NOTHING;
    INSERT INTO audit.access_log(tenant_id,actor_id,location_id,action,appointment_id,specialist_id,visit_date,after_id,page_limit,include_cancelled,result_count)
        VALUES(p_tenant,actor,p_location,p_action,p_appointment,p_specialist,visit,p_after,p_limit,p_cancelled,p_count);
END $f$;
REVOKE ALL ON FUNCTION audit.record_access(uuid,uuid,text,uuid,uuid,text,uuid,integer,boolean,integer) FROM PUBLIC,turnesta_provisioner,turnesta_delivery_worker;
GRANT EXECUTE ON FUNCTION audit.record_access(uuid,uuid,text,uuid,uuid,text,uuid,integer,boolean,integer) TO turnesta_runtime;
