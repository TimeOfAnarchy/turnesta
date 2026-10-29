-- Queue/Tips is an optional link, not a payment or a historical attribution.
ALTER TABLE queue.queues ADD CONSTRAINT queues_point_key UNIQUE (tenant_id, id, location_id);
CREATE TABLE queue.tip_settings (
    tenant_id uuid NOT NULL,
    queue_id uuid NOT NULL,
    location_id uuid NOT NULL,
    recipient_id uuid,
    version integer NOT NULL DEFAULT 1 CHECK (version > 0),
    PRIMARY KEY (tenant_id, queue_id),
    FOREIGN KEY (tenant_id, queue_id, location_id) REFERENCES queue.queues (tenant_id, id, location_id),
    FOREIGN KEY (tenant_id, recipient_id, location_id) REFERENCES tips.recipients (tenant_id, id, location_id)
);
REVOKE ALL ON queue.tip_settings FROM PUBLIC;
GRANT SELECT, INSERT, UPDATE ON queue.tip_settings TO turnesta_runtime;
ALTER TABLE queue.tip_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE queue.tip_settings FORCE ROW LEVEL SECURITY;
CREATE POLICY manager_access ON queue.tip_settings TO turnesta_runtime USING (
    tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
    AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = tip_settings.tenant_id
        AND m.user_id = nullif(current_setting('app.user_id', true), '')::uuid AND m.role IN ('owner', 'manager'))
) WITH CHECK (
    tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
    AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = tip_settings.tenant_id
        AND m.user_id = nullif(current_setting('app.user_id', true), '')::uuid AND m.role IN ('owner', 'manager'))
);
CREATE POLICY provisioner ON queue.tip_settings TO turnesta_provisioner USING (true) WITH CHECK (true);
CREATE TRIGGER queue_tip_events AFTER INSERT OR UPDATE ON queue.tip_settings
    FOR EACH STATEMENT EXECUTE FUNCTION queue.notify_change();

CREATE OR REPLACE FUNCTION queue.live_states(p_tenants uuid[], p_queues uuid[])
RETURNS TABLE (tenant_id uuid, queue_id uuid, revision text, public_slug uuid, display_enabled boolean)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $function$
    SELECT q.tenant_id, q.id,
        concat_ws(':', q.version, coalesce(s.version,0), l.version, o.version, coalesce(ts.version,0)),
        s.public_slug, coalesce(s.display_enabled,false)
    FROM unnest(p_tenants,p_queues) AS target(tenant_id,queue_id)
    JOIN queue.queues q ON q.tenant_id=target.tenant_id AND q.id=target.queue_id
    JOIN core.locations l ON l.tenant_id=q.tenant_id AND l.id=q.location_id
    JOIN core.organizations o ON o.id=q.tenant_id
    LEFT JOIN queue.public_settings s ON s.tenant_id=q.tenant_id AND s.queue_id=q.id
    LEFT JOIN queue.tip_settings ts ON ts.tenant_id=q.tenant_id AND ts.queue_id=q.id
    WHERE cardinality(p_tenants)=cardinality(p_queues) AND cardinality(p_tenants)<=128
$function$;

CREATE FUNCTION queue.public_ticket_with_tips(p_slug uuid, p_hash bytea)
RETURNS jsonb LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $function$
    SELECT to_jsonb(ticket) || jsonb_build_object('tips', CASE
        WHEN ticket.status = 'completed' AND r.environment = 'mock' THEN
            jsonb_build_object('public_slug', r.public_slug, 'display_name', r.display_name, 'environment', r.environment)
        ELSE NULL END)
    FROM queue.public_ticket(p_slug, p_hash) ticket
    JOIN queue.public_settings s ON s.public_slug = p_slug
    LEFT JOIN queue.tip_settings ts ON ts.tenant_id=s.tenant_id AND ts.queue_id=s.queue_id
    LEFT JOIN tips.recipients r ON r.tenant_id=ts.tenant_id AND r.id=ts.recipient_id AND r.location_id=ts.location_id
$function$;
REVOKE ALL ON FUNCTION queue.public_ticket_with_tips(uuid,bytea) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION queue.public_ticket_with_tips(uuid,bytea) TO turnesta_runtime;
