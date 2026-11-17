-- Serialize expired-key checks under the same isolation level as runtime commands.
CREATE OR REPLACE FUNCTION retention.guard_key() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE row_data jsonb; v_kind text:=TG_TABLE_SCHEMA||'.'||TG_TABLE_NAME; v_tenant uuid; v_scope uuid; v_actor text; v_key text; lock_key bigint; state text;
BEGIN
    IF current_setting('transaction_isolation') <> 'read committed' THEN RAISE EXCEPTION 'retention requires read committed isolation' USING ERRCODE='25001'; END IF;
    row_data:=CASE WHEN TG_OP='DELETE' THEN to_jsonb(OLD) ELSE to_jsonb(NEW) END;
    v_tenant:=(row_data->>'tenant_id')::uuid;
    v_scope:=coalesce((row_data->>'queue_id')::uuid,(row_data->>'location_id')::uuid,v_tenant);
    v_actor:=coalesce(row_data->>'actor_id','');
    v_key:=coalesce(row_data->>'idempotency_key',row_data->>'key',row_data->>'token_hash');
    lock_key:=hashtextextended(v_kind||':'||v_tenant::text||':'||v_scope::text||':'||v_actor||':'||v_key,0);
    IF TG_OP='DELETE' THEN
        -- Skip a key held by an in-flight command instead of deadlocking.
        IF NOT pg_try_advisory_xact_lock(lock_key) THEN
            IF session_user='turnesta_migrator' AND current_setting('retention.purging',true)='true' THEN RETURN NULL; END IF;
            RAISE EXCEPTION 'key is in use; retry transaction' USING ERRCODE='40001';
        END IF;
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

CREATE OR REPLACE FUNCTION retention.purge_batch(p_kind text,p_dry_run boolean) RETURNS bigint
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
    IF NOT p_dry_run THEN PERFORM set_config('retention.purging','true',true); END IF;
    IF p_dry_run THEN
        sql:=format('SELECT count(*) FROM %s WHERE %s',relation,predicate);
    ELSE
        sql:=format('WITH victims AS (SELECT ctid FROM %s WHERE %s ORDER BY ctid LIMIT 1000 FOR UPDATE SKIP LOCKED), deleted AS (DELETE FROM %s d USING victims v WHERE d.ctid=v.ctid RETURNING 1) SELECT count(*) FROM deleted',relation,predicate,relation);
    END IF;
    EXECUTE sql INTO n;
    RETURN n;
END $f$;
