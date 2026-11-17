-- Queue commands inspect their receipt before performing the mutation. Check
-- expired markers there as well, so a closed ticket cannot mask the conflict.
GRANT USAGE ON SCHEMA retention TO turnesta_runtime;
CREATE FUNCTION retention.queue_key_expired(p_tenant uuid,p_queue uuid,p_actor uuid,p_key uuid)
RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $f$
BEGIN
    IF p_tenant IS DISTINCT FROM nullif(current_setting('app.tenant_id',true),'')::uuid
        OR p_actor IS DISTINCT FROM nullif(current_setting('app.user_id',true),'')::uuid
        OR NOT EXISTS(SELECT 1 FROM core.memberships m WHERE m.tenant_id=p_tenant AND m.user_id=p_actor AND m.active AND m.role IN ('owner','manager','operator'))
        OR NOT EXISTS(SELECT 1 FROM queue.queues q WHERE q.tenant_id=p_tenant AND q.id=p_queue)
    THEN RAISE EXCEPTION 'queue access denied' USING ERRCODE='42501'; END IF;
    RETURN EXISTS(SELECT 1 FROM retention.used_keys t WHERE t.kind='queue.commands' AND t.tenant_id=p_tenant AND t.scope_id=p_queue AND t.actor_id=p_actor::text AND t.key_value=p_key::text);
END $f$;
REVOKE ALL ON FUNCTION retention.queue_key_expired(uuid,uuid,uuid,uuid) FROM PUBLIC,turnesta_provisioner,turnesta_delivery_worker;
GRANT EXECUTE ON FUNCTION retention.queue_key_expired(uuid,uuid,uuid,uuid) TO turnesta_runtime;
