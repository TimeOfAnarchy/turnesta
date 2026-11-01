CREATE TABLE queue.stations (
    tenant_id uuid NOT NULL,
    queue_id uuid NOT NULL,
    id uuid NOT NULL,
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 120 AND name=btrim(name) AND name !~ '[[:cntrl:]]'),
    enabled boolean NOT NULL DEFAULT true,
    version integer NOT NULL DEFAULT 1 CHECK (version>0),
    PRIMARY KEY (tenant_id,queue_id,id),
    FOREIGN KEY (tenant_id,queue_id) REFERENCES queue.queues (tenant_id,id)
);
ALTER TABLE queue.stations ENABLE ROW LEVEL SECURITY;
ALTER TABLE queue.stations FORCE ROW LEVEL SECURITY;
CREATE POLICY provisioner ON queue.stations TO turnesta_provisioner USING (true) WITH CHECK (true);
CREATE POLICY tenant_read ON queue.stations FOR SELECT TO turnesta_runtime USING (
    tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
    AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id=stations.tenant_id
        AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid)
);
CREATE POLICY manager_insert ON queue.stations FOR INSERT TO turnesta_runtime WITH CHECK (
    tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
    AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id=stations.tenant_id
        AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.role IN ('owner','manager'))
);
CREATE POLICY manager_update ON queue.stations FOR UPDATE TO turnesta_runtime USING (
    tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
    AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id=stations.tenant_id
        AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.role IN ('owner','manager'))
) WITH CHECK (
    tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
    AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id=stations.tenant_id
        AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.role IN ('owner','manager'))
);
REVOKE ALL ON queue.stations FROM PUBLIC;
GRANT SELECT,INSERT ON queue.stations TO turnesta_runtime;
GRANT UPDATE (name,enabled,version) ON queue.stations TO turnesta_runtime;

ALTER TABLE queue.tickets ADD COLUMN station_id uuid;
ALTER TABLE queue.tickets ADD COLUMN station_name text;
ALTER TABLE queue.tickets ADD CONSTRAINT ticket_station_pair CHECK ((station_id IS NULL)=(station_name IS NULL));
ALTER TABLE queue.tickets ADD CONSTRAINT ticket_station_called CHECK (station_id IS NULL OR called_at IS NOT NULL);
ALTER TABLE queue.tickets ADD CONSTRAINT ticket_station_link FOREIGN KEY (tenant_id,queue_id,station_id)
    REFERENCES queue.stations (tenant_id,queue_id,id);
CREATE UNIQUE INDEX tickets_station_active_idx ON queue.tickets (tenant_id,queue_id,station_id)
    WHERE status='called' AND station_id IS NOT NULL;
CREATE OR REPLACE VIEW queue.ticket_details WITH (security_invoker=true) AS
SELECT tenant_id,queue_id,id,position,to_char(service_date,'YYYY-MM-DD') AS service_date,number,status,called_by,
    to_char(issued_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS issued_at,
    to_char(called_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS called_at,
    to_char(closed_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS closed_at,
    service_id,service_name,station_id,station_name
FROM queue.tickets;

-- Keep the existing service-aware entry point so legacy issue/replay callers
-- receive the same capability/TTL/Tips checks and the new safe projection.
CREATE OR REPLACE FUNCTION queue.public_ticket_with_service(p_slug uuid,p_hash bytea)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $function$
DECLARE v_ticket jsonb; v_service text; v_station text;
BEGIN
    v_ticket:=queue.public_ticket_with_tips(p_slug,p_hash);
    SELECT t.service_name,t.station_name INTO v_service,v_station FROM queue.public_settings s JOIN queue.guest_claims c
        ON c.tenant_id=s.tenant_id AND c.queue_id=s.queue_id
        JOIN queue.tickets t ON t.tenant_id=c.tenant_id AND t.queue_id=c.queue_id AND t.id=c.ticket_id
        WHERE s.public_slug=p_slug AND c.token_hash=p_hash;
    RETURN v_ticket || jsonb_build_object('service_name',v_service,'station_name',v_station);
END
$function$;

CREATE OR REPLACE FUNCTION queue.public_display(p_slug uuid,p_page integer)
RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $function$
    SELECT jsonb_build_object(
        'queue_name',q.name,'location_name',l.name,'organization_name',o.name,
        'time_zone',l.time_zone,'version',q.version::text,
        'observed_at',to_char(statement_timestamp() AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
        'join_open',s.join_open,'waiting_count',totals.waiting_count,
        'called_count',totals.called_count,'page',p_page,
        'next_page',CASE WHEN totals.called_count>(p_page::bigint+1)*100 THEN p_page+1 ELSE NULL END,
        'called',coalesce((SELECT jsonb_agg(jsonb_build_object(
            'number',t.number,'service_date',to_char(t.service_date,'YYYY-MM-DD'),'station_name',t.station_name) ORDER BY t.position)
            FROM (SELECT number,service_date,position,station_name FROM queue.tickets
                WHERE tenant_id=q.tenant_id AND queue_id=q.id AND status='called'
                ORDER BY position LIMIT 100 OFFSET p_page::bigint*100) t),'[]'::jsonb)
    )
    FROM queue.public_settings s
    JOIN queue.queues q ON q.tenant_id=s.tenant_id AND q.id=s.queue_id
    JOIN core.locations l ON l.tenant_id=q.tenant_id AND l.id=q.location_id
    JOIN core.organizations o ON o.id=q.tenant_id
    CROSS JOIN LATERAL (SELECT count(*) FILTER (WHERE status='waiting') AS waiting_count,
        count(*) FILTER (WHERE status='called') AS called_count FROM queue.tickets
        WHERE tenant_id=q.tenant_id AND queue_id=q.id AND status IN ('waiting','called')) totals
    WHERE s.public_slug=p_slug AND s.display_enabled AND p_page BETWEEN 0 AND 21474835
$function$;
