-- Notifications are wake-ups, not a durable event log. The row versions remain
-- authoritative and periodic reconciliation repairs missed notifications.
CREATE FUNCTION queue.notify_change() RETURNS trigger LANGUAGE plpgsql
SET search_path = pg_catalog AS $function$
BEGIN
    PERFORM pg_notify('turnesta_queue', 'changed');
    RETURN NULL;
END
$function$;
REVOKE ALL ON FUNCTION queue.notify_change() FROM PUBLIC;
CREATE TRIGGER queue_events AFTER UPDATE OF version ON queue.queues
    FOR EACH STATEMENT EXECUTE FUNCTION queue.notify_change();
CREATE TRIGGER queue_public_events AFTER INSERT OR UPDATE ON queue.public_settings
    FOR EACH STATEMENT EXECUTE FUNCTION queue.notify_change();
CREATE TRIGGER queue_location_events AFTER UPDATE OF version ON core.locations
    FOR EACH STATEMENT EXECUTE FUNCTION queue.notify_change();
CREATE TRIGGER queue_organization_events AFTER UPDATE OF version ON core.organizations
    FOR EACH STATEMENT EXECUTE FUNCTION queue.notify_change();

CREATE FUNCTION queue.live_states(p_tenants uuid[], p_queues uuid[])
RETURNS TABLE (tenant_id uuid, queue_id uuid, revision text, public_slug uuid, display_enabled boolean)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $function$
    SELECT q.tenant_id, q.id,
        concat_ws(':', q.version, coalesce(s.version,0), l.version, o.version),
        s.public_slug, coalesce(s.display_enabled,false)
    FROM unnest(p_tenants,p_queues) AS target(tenant_id,queue_id)
    JOIN queue.queues q ON q.tenant_id=target.tenant_id AND q.id=target.queue_id
    JOIN core.locations l ON l.tenant_id=q.tenant_id AND l.id=q.location_id
    JOIN core.organizations o ON o.id=q.tenant_id
    LEFT JOIN queue.public_settings s ON s.tenant_id=q.tenant_id AND s.queue_id=q.id
    WHERE cardinality(p_tenants)=cardinality(p_queues) AND cardinality(p_tenants)<=128
$function$;
REVOKE ALL ON FUNCTION queue.live_states(uuid[],uuid[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION queue.live_states(uuid[],uuid[]) TO turnesta_runtime;

CREATE FUNCTION queue.public_stream_target(p_slug uuid, p_display boolean)
RETURNS TABLE (tenant_id uuid, queue_id uuid, revision text, public_slug uuid, display_enabled boolean)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $function$
    SELECT state.* FROM queue.public_settings s
    CROSS JOIN LATERAL queue.live_states(ARRAY[s.tenant_id],ARRAY[s.queue_id]) state
    WHERE s.public_slug=p_slug AND (NOT p_display OR s.display_enabled)
$function$;
REVOKE ALL ON FUNCTION queue.public_stream_target(uuid,boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION queue.public_stream_target(uuid,boolean) TO turnesta_runtime;
