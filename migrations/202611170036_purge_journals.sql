-- Only compact key markers survive journal and capability cleanup.
CREATE SCHEMA retention AUTHORIZATION CURRENT_USER;
REVOKE ALL ON SCHEMA retention FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;
CREATE TABLE retention.used_keys (
    kind text NOT NULL, tenant_id uuid NOT NULL, scope_id uuid NOT NULL,
    actor_id text NOT NULL, key_value text NOT NULL,
    PRIMARY KEY(kind,tenant_id,scope_id,actor_id,key_value)
);
REVOKE ALL ON retention.used_keys FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;

CREATE FUNCTION retention.guard_key() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE row_data jsonb; v_kind text:=TG_TABLE_SCHEMA||'.'||TG_TABLE_NAME; v_tenant uuid; v_scope uuid; v_actor text; v_key text; lock_key bigint; state text;
BEGIN
    row_data:=CASE WHEN TG_OP='DELETE' THEN to_jsonb(OLD) ELSE to_jsonb(NEW) END;
    v_tenant:=(row_data->>'tenant_id')::uuid;
    v_scope:=coalesce((row_data->>'queue_id')::uuid,(row_data->>'location_id')::uuid,v_tenant);
    v_actor:=coalesce(row_data->>'actor_id','');
    v_key:=coalesce(row_data->>'idempotency_key',row_data->>'key',row_data->>'token_hash');
    lock_key:=hashtextextended(v_kind||':'||v_tenant::text||':'||v_scope::text||':'||v_actor||':'||v_key,0);
    IF TG_OP='DELETE' THEN
        -- Skip a key held by an in-flight command instead of deadlocking.
        IF NOT pg_try_advisory_xact_lock(lock_key) THEN RETURN NULL; END IF;
        INSERT INTO retention.used_keys VALUES(v_kind,v_tenant,v_scope,v_actor,v_key) ON CONFLICT DO NOTHING;
        RETURN OLD;
    END IF;
    PERFORM pg_advisory_xact_lock(lock_key);
    IF EXISTS(SELECT 1 FROM retention.used_keys t WHERE t.kind=v_kind AND t.tenant_id=v_tenant AND t.scope_id=v_scope AND t.actor_id=v_actor AND t.key_value=v_key) THEN
        state:=CASE v_kind WHEN 'queue.guest_claims' THEN 'Q2003' WHEN 'bookings.public_claims' THEN 'B2207' WHEN 'core.team_commands' THEN 'M2202' ELSE 'R1401' END;
        RAISE EXCEPTION 'expired key cannot be reused' USING ERRCODE=state;
    END IF;
    RETURN NEW;
END $f$;
REVOKE ALL ON FUNCTION retention.guard_key() FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;

CREATE POLICY retention_owner ON queue.commands TO CURRENT_USER USING(true) WITH CHECK(true);
CREATE POLICY retention_owner ON bookings.commands TO CURRENT_USER USING(true) WITH CHECK(true);
CREATE POLICY retention_owner ON bookings.appointment_commands TO CURRENT_USER USING(true) WITH CHECK(true);

CREATE TRIGGER retain_key BEFORE INSERT OR DELETE ON queue.commands FOR EACH ROW EXECUTE FUNCTION retention.guard_key();
CREATE TRIGGER retain_key BEFORE INSERT OR DELETE ON bookings.commands FOR EACH ROW EXECUTE FUNCTION retention.guard_key();
CREATE TRIGGER retain_key BEFORE INSERT OR DELETE ON bookings.appointment_commands FOR EACH ROW EXECUTE FUNCTION retention.guard_key();
CREATE TRIGGER retain_key BEFORE INSERT OR DELETE ON core.team_commands FOR EACH ROW EXECUTE FUNCTION retention.guard_key();
CREATE TRIGGER retain_key BEFORE INSERT OR DELETE ON queue.guest_claims FOR EACH ROW EXECUTE FUNCTION retention.guard_key();
CREATE TRIGGER retain_key BEFORE INSERT OR DELETE ON bookings.public_claims FOR EACH ROW EXECUTE FUNCTION retention.guard_key();

CREATE FUNCTION retention.purge_batch(p_kind text,p_dry_run boolean) RETURNS bigint
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE predicate text; relation regclass; n bigint; sql text;
BEGIN
    IF session_user<>'turnesta_migrator' OR p_dry_run IS NULL THEN RAISE EXCEPTION 'maintenance login required' USING ERRCODE='42501'; END IF;
    CASE p_kind
        WHEN 'queue_commands' THEN relation:='queue.commands'::regclass; predicate:='created_at < clock_timestamp()-interval ''180 days''';
        WHEN 'booking_commands' THEN relation:='bookings.commands'::regclass; predicate:='created_at < clock_timestamp()-interval ''180 days''';
        WHEN 'appointment_commands' THEN relation:='bookings.appointment_commands'::regclass; predicate:='created_at < clock_timestamp()-interval ''180 days''';
        WHEN 'team_commands' THEN relation:='core.team_commands'::regclass; predicate:='created_at < clock_timestamp()-interval ''180 days''';
        WHEN 'public_claims' THEN relation:='bookings.public_claims'::regclass; predicate:='expires_at < clock_timestamp()';
        WHEN 'guest_claims' THEN relation:='queue.guest_claims'::regclass; predicate:='EXISTS (SELECT 1 FROM queue.tickets t WHERE t.tenant_id=queue.guest_claims.tenant_id AND t.queue_id=queue.guest_claims.queue_id AND t.id=queue.guest_claims.ticket_id AND t.closed_at < clock_timestamp()-interval ''24 hours'')';
        ELSE RAISE EXCEPTION 'unknown retention class' USING ERRCODE='22023';
    END CASE;
    IF p_dry_run THEN
        sql:=format('SELECT count(*) FROM %s WHERE %s',relation,predicate);
    ELSE
        sql:=format('WITH victims AS (SELECT ctid FROM %s WHERE %s ORDER BY ctid LIMIT 1000 FOR UPDATE SKIP LOCKED), deleted AS (DELETE FROM %s d USING victims v WHERE d.ctid=v.ctid RETURNING 1) SELECT count(*) FROM deleted',relation,predicate,relation);
    END IF;
    EXECUTE sql INTO n;
    RETURN n;
END $f$;
REVOKE ALL ON FUNCTION retention.purge_batch(text,boolean) FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;

CREATE OR REPLACE FUNCTION queue.public_ticket(p_slug uuid, p_hash bytea)
RETURNS TABLE (number integer, service_date text, status text, issued_at text, people_ahead bigint)
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $function$
DECLARE v_closed timestamptz;
BEGIN
    IF octet_length(p_hash) IS DISTINCT FROM 32 THEN
        RAISE EXCEPTION 'invalid token' USING ERRCODE = 'Q2002';
    END IF;
    SELECT t.closed_at INTO v_closed FROM queue.guest_claims c
        JOIN queue.public_settings s ON s.tenant_id = c.tenant_id AND s.queue_id = c.queue_id
        JOIN queue.tickets t ON t.tenant_id = c.tenant_id AND t.queue_id = c.queue_id AND t.id = c.ticket_id
        WHERE s.public_slug = p_slug AND c.token_hash = p_hash;
    IF NOT FOUND THEN
        IF EXISTS(SELECT 1 FROM queue.public_settings s JOIN retention.used_keys t ON t.kind='queue.guest_claims' AND t.tenant_id=s.tenant_id AND t.scope_id=s.queue_id AND t.actor_id='' AND t.key_value=to_jsonb(p_hash)#>>'{}' WHERE s.public_slug=p_slug) THEN
            RAISE EXCEPTION 'public ticket expired' USING ERRCODE='Q2003';
        END IF;
        RAISE EXCEPTION 'ticket not found' USING ERRCODE='Q2001';
    END IF;
    IF v_closed < clock_timestamp() - interval '24 hours' THEN
        RAISE EXCEPTION 'public ticket expired' USING ERRCODE = 'Q2003';
    END IF;
    RETURN QUERY SELECT t.number, to_char(t.service_date, 'YYYY-MM-DD'), t.status,
        to_char(t.issued_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
        CASE WHEN t.status = 'waiting' THEN (SELECT count(*) FROM queue.tickets ahead
            WHERE ahead.tenant_id = t.tenant_id AND ahead.queue_id = t.queue_id
                AND ahead.status = 'waiting' AND ahead.position < t.position) ELSE NULL::bigint END
    FROM queue.guest_claims c JOIN queue.public_settings s ON s.tenant_id = c.tenant_id AND s.queue_id = c.queue_id
    JOIN queue.tickets t ON t.tenant_id = c.tenant_id AND t.queue_id = c.queue_id AND t.id = c.ticket_id
    WHERE s.public_slug = p_slug AND c.token_hash = p_hash;
END
$function$;

CREATE OR REPLACE FUNCTION bookings.public_receipt(p_slug uuid,p_hash bytea) RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $f$
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
    IF NOT FOUND THEN
        IF EXISTS(SELECT 1 FROM bookings.public_settings s JOIN retention.used_keys t ON t.kind='bookings.public_claims' AND t.tenant_id=s.tenant_id AND t.scope_id=s.location_id AND t.actor_id='' AND t.key_value=to_jsonb(p_hash)#>>'{}' WHERE s.public_slug=p_slug) THEN
            RAISE EXCEPTION 'receipt expired' USING ERRCODE='B2207';
        END IF;
        RAISE EXCEPTION 'receipt not found' USING ERRCODE='B2201';
    END IF;
    IF expiry<=current_timestamp THEN RAISE EXCEPTION 'receipt expired' USING ERRCODE='B2207'; END IF;
    RETURN receipt;
END $f$;


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
    IF EXISTS(SELECT 1 FROM retention.used_keys t WHERE t.kind='bookings.public_claims' AND t.tenant_id=settings.tenant_id AND t.scope_id=settings.location_id AND t.actor_id='' AND t.key_value=to_jsonb(p_hash)#>>'{}') THEN RAISE EXCEPTION 'receipt expired' USING ERRCODE='B2207'; END IF;
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
        OR (SELECT count(*) FROM bookings.appointments a WHERE a.tenant_id=settings.tenant_id AND a.location_id=settings.location_id AND a.source='public' AND a.status='confirmed' AND (a.starts_at>clock_timestamp() OR (a.starts_at IS NULL AND (a.local_date IS NULL OR a.local_date>=(clock_timestamp() AT TIME ZONE point_zone)::date))))>=1000
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

CREATE OR REPLACE FUNCTION queue.public_issue(p_slug uuid, p_hash bytea)
RETURNS TABLE (number integer, service_date text, status text, issued_at text, people_ahead bigint)
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $function$
DECLARE v_tenant uuid; v_queue uuid; v_open boolean; v_ticket uuid; v_count bigint; v_now timestamptz;
BEGIN
    IF octet_length(p_hash) IS DISTINCT FROM 32 THEN
        RAISE EXCEPTION 'invalid token' USING ERRCODE = 'Q2002';
    END IF;
    SELECT s.tenant_id, s.queue_id INTO v_tenant, v_queue FROM queue.public_settings s WHERE s.public_slug = p_slug;
    IF NOT FOUND THEN RAISE EXCEPTION 'queue not found' USING ERRCODE = 'Q2001'; END IF;
    PERFORM 1 FROM queue.queues WHERE tenant_id = v_tenant AND id = v_queue FOR UPDATE;
    IF EXISTS(SELECT 1 FROM retention.used_keys t WHERE t.kind='queue.guest_claims' AND t.tenant_id=v_tenant AND t.scope_id=v_queue AND t.actor_id='' AND t.key_value=to_jsonb(p_hash)#>>'{}') THEN RAISE EXCEPTION 'public ticket expired' USING ERRCODE='Q2003'; END IF;
    -- Manager changes use the same queue lock; read the setting after waiting.
    SELECT s.join_open INTO v_open FROM queue.public_settings s WHERE s.public_slug = p_slug;
    IF EXISTS (SELECT 1 FROM queue.guest_claims c
        WHERE c.tenant_id = v_tenant AND c.queue_id = v_queue AND c.token_hash = p_hash) THEN
        RETURN QUERY SELECT * FROM queue.public_ticket(p_slug, p_hash);
        RETURN;
    END IF;
    IF NOT v_open THEN RAISE EXCEPTION 'public joining closed' USING ERRCODE = 'Q2004'; END IF;
    v_now := clock_timestamp();
    SELECT count(*) INTO v_count FROM (SELECT 1 FROM queue.guest_claims c
        WHERE c.tenant_id = v_tenant AND c.queue_id = v_queue
            AND c.created_at > v_now - interval '1 minute' LIMIT 30) recent;
    IF v_count >= 30 THEN RAISE EXCEPTION 'public issue rate limit' USING ERRCODE = 'Q2005'; END IF;
    SELECT count(*) INTO v_count FROM (SELECT 1 FROM queue.tickets t
        WHERE t.tenant_id = v_tenant AND t.queue_id = v_queue AND t.status = 'waiting' LIMIT 500) waiting;
    IF v_count >= 500 THEN RAISE EXCEPTION 'queue capacity limit' USING ERRCODE = 'Q2006'; END IF;
    v_ticket := queue.allocate_ticket(v_tenant, v_queue);
    INSERT INTO queue.guest_claims (tenant_id, queue_id, token_hash, ticket_id)
        VALUES (v_tenant, v_queue, p_hash, v_ticket);
    UPDATE queue.queues SET version = version + 1 WHERE tenant_id = v_tenant AND id = v_queue;
    RETURN QUERY SELECT * FROM queue.public_ticket(p_slug, p_hash);
END
$function$;

CREATE OR REPLACE FUNCTION queue.public_issue(p_slug uuid,p_hash bytea,p_service uuid)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $function$
DECLARE v_tenant uuid; v_queue uuid; v_open boolean; v_ticket uuid; v_count bigint; v_now timestamptz;
BEGIN
    IF octet_length(p_hash) IS DISTINCT FROM 32 THEN RAISE EXCEPTION 'invalid token' USING ERRCODE='Q2002'; END IF;
    SELECT s.tenant_id,s.queue_id INTO v_tenant,v_queue FROM queue.public_settings s WHERE s.public_slug=p_slug;
    IF NOT FOUND THEN RAISE EXCEPTION 'queue not found' USING ERRCODE='Q2001'; END IF;
    PERFORM 1 FROM queue.queues WHERE tenant_id=v_tenant AND id=v_queue FOR UPDATE;
    IF EXISTS(SELECT 1 FROM retention.used_keys t WHERE t.kind='queue.guest_claims' AND t.tenant_id=v_tenant AND t.scope_id=v_queue AND t.actor_id='' AND t.key_value=to_jsonb(p_hash)#>>'{}') THEN RAISE EXCEPTION 'public ticket expired' USING ERRCODE='Q2003'; END IF;
    IF EXISTS (SELECT 1 FROM queue.guest_claims c WHERE c.tenant_id=v_tenant AND c.queue_id=v_queue AND c.token_hash=p_hash) THEN
        RETURN queue.public_ticket_with_service(p_slug,p_hash);
    END IF;
    SELECT s.join_open INTO v_open FROM queue.public_settings s WHERE s.public_slug=p_slug;
    IF NOT v_open THEN RAISE EXCEPTION 'public joining closed' USING ERRCODE='Q2004'; END IF;
    v_now:=clock_timestamp();
    SELECT count(*) INTO v_count FROM (SELECT 1 FROM queue.guest_claims c WHERE c.tenant_id=v_tenant AND c.queue_id=v_queue
        AND c.created_at>v_now-interval '1 minute' LIMIT 30) recent;
    IF v_count>=30 THEN RAISE EXCEPTION 'public issue rate limit' USING ERRCODE='Q2005'; END IF;
    SELECT count(*) INTO v_count FROM (SELECT 1 FROM queue.tickets t WHERE t.tenant_id=v_tenant AND t.queue_id=v_queue
        AND t.status='waiting' LIMIT 500) waiting;
    IF v_count>=500 THEN RAISE EXCEPTION 'queue capacity limit' USING ERRCODE='Q2006'; END IF;
    v_ticket:=queue.allocate_ticket(v_tenant,v_queue,p_service);
    INSERT INTO queue.guest_claims (tenant_id,queue_id,token_hash,ticket_id) VALUES (v_tenant,v_queue,p_hash,v_ticket);
    UPDATE queue.queues SET version=version+1 WHERE tenant_id=v_tenant AND id=v_queue;
    RETURN queue.public_ticket_with_service(p_slug,p_hash);
END
$function$;
