CREATE OR REPLACE FUNCTION bookings.retention_batch(p_dry_run boolean) RETURNS bigint
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE
    org record;
    calendar record;
    visit record;
    cutoff timestamptz := clock_timestamp();
    deadline timestamptz;
    processed bigint := 0;
BEGIN
    IF session_user <> 'turnesta_migrator' OR p_dry_run IS NULL THEN
        RAISE EXCEPTION 'maintenance login required' USING ERRCODE='42501';
    END IF;
    IF p_dry_run THEN
        SELECT count(*) INTO processed FROM bookings.appointments a JOIN core.organizations o ON o.id=a.tenant_id
        WHERE o.booking_retention_months IS NOT NULL AND a.anonymized_at IS NULL AND
            (CASE WHEN a.status='cancelled' THEN a.cancelled_at ELSE a.ends_at END) < cutoff-make_interval(months=>o.booking_retention_months);
        RETURN processed;
    END IF;

    FOR org IN
        SELECT o.id,o.booking_retention_months FROM core.organizations o
        WHERE o.booking_retention_months IS NOT NULL AND EXISTS (
            SELECT 1 FROM bookings.appointments a WHERE a.tenant_id=o.id AND a.anonymized_at IS NULL
            AND (CASE WHEN a.status='cancelled' THEN a.cancelled_at ELSE a.ends_at END) < cutoff-make_interval(months=>o.booking_retention_months)
        ) ORDER BY o.id FOR SHARE OF o
    LOOP
        deadline := cutoff-make_interval(months=>org.booking_retention_months);
        PERFORM set_config('app.tenant_id',org.id::text,true),set_config('app.user_id','',true);
        -- Take the same calendar lock as booking commands before counting any visit towards the batch.
        FOR calendar IN
            SELECT c.location_id,c.specialist_id FROM bookings.calendars c
            WHERE c.tenant_id=org.id AND EXISTS (
                SELECT 1 FROM bookings.appointments a WHERE a.tenant_id=c.tenant_id AND a.location_id=c.location_id
                    AND a.specialist_id=c.specialist_id AND a.anonymized_at IS NULL
                    AND (CASE WHEN a.status='cancelled' THEN a.cancelled_at ELSE a.ends_at END) < deadline
            ) ORDER BY c.location_id,c.specialist_id FOR UPDATE OF c SKIP LOCKED
        LOOP
            FOR visit IN
                SELECT a.id,a.version FROM bookings.appointments a WHERE a.tenant_id=org.id AND a.location_id=calendar.location_id
                    AND a.specialist_id=calendar.specialist_id AND a.anonymized_at IS NULL
                    AND (CASE WHEN a.status='cancelled' THEN a.cancelled_at ELSE a.ends_at END) < deadline
                ORDER BY a.id LIMIT (1000-processed)
            LOOP
                PERFORM bookings.erase_appointment_fields(org.id,calendar.location_id,visit.id,visit.version);
                processed := processed+1;
            END LOOP;
            IF processed=1000 THEN RETURN processed; END IF;
        END LOOP;
    END LOOP;
    RETURN processed;
END $f$;
