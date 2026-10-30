CREATE SCHEMA catalog;
REVOKE ALL ON SCHEMA catalog FROM PUBLIC;
GRANT USAGE ON SCHEMA catalog TO turnesta_runtime;
CREATE TABLE catalog.services (
    tenant_id uuid NOT NULL,
    id uuid NOT NULL,
    location_id uuid NOT NULL,
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 120 AND name=btrim(name) AND name !~ '[[:cntrl:]]'),
    description text NOT NULL DEFAULT '' CHECK (char_length(description)<=500 AND description=btrim(description) AND description !~ '[[:cntrl:]]'),
    enabled boolean NOT NULL DEFAULT true,
    version integer NOT NULL DEFAULT 1 CHECK (version>0),
    PRIMARY KEY (tenant_id,id),
    UNIQUE (tenant_id,id,location_id),
    FOREIGN KEY (tenant_id,location_id) REFERENCES core.locations (tenant_id,id)
);
CREATE TABLE queue.service_links (
    tenant_id uuid NOT NULL,
    queue_id uuid NOT NULL,
    location_id uuid NOT NULL,
    service_id uuid NOT NULL,
    PRIMARY KEY (tenant_id,queue_id,service_id),
    UNIQUE (tenant_id,service_id),
    FOREIGN KEY (tenant_id,queue_id,location_id) REFERENCES queue.queues (tenant_id,id,location_id),
    FOREIGN KEY (tenant_id,service_id,location_id) REFERENCES catalog.services (tenant_id,id,location_id)
);
ALTER TABLE catalog.services ENABLE ROW LEVEL SECURITY;
ALTER TABLE catalog.services FORCE ROW LEVEL SECURITY;
ALTER TABLE queue.service_links ENABLE ROW LEVEL SECURITY;
ALTER TABLE queue.service_links FORCE ROW LEVEL SECURITY;
DO $policies$
DECLARE target text;
BEGIN
    FOREACH target IN ARRAY ARRAY['catalog.services','queue.service_links'] LOOP
        EXECUTE format('CREATE POLICY provisioner ON %s TO turnesta_provisioner USING (true) WITH CHECK (true)',target);
        EXECUTE format($sql$CREATE POLICY tenant_read ON %s FOR SELECT TO turnesta_runtime USING (
            tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
            AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id=%I.tenant_id
                AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid))$sql$,target,split_part(target,'.',2));
        EXECUTE format($sql$CREATE POLICY manager_write ON %s FOR INSERT TO turnesta_runtime WITH CHECK (
            tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
            AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
                AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.role IN ('owner','manager')))$sql$,target);
    END LOOP;
END
$policies$;
CREATE POLICY manager_update ON catalog.services FOR UPDATE TO turnesta_runtime USING (
    tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
    AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id=services.tenant_id
        AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.role IN ('owner','manager'))
) WITH CHECK (
    tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
    AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id=services.tenant_id
        AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.role IN ('owner','manager'))
);
REVOKE ALL ON catalog.services,queue.service_links FROM PUBLIC;
GRANT SELECT,INSERT ON catalog.services,queue.service_links TO turnesta_runtime;
GRANT UPDATE (name,description,enabled,version) ON catalog.services TO turnesta_runtime;

ALTER TABLE queue.tickets ADD COLUMN service_id uuid;
ALTER TABLE queue.tickets ADD COLUMN service_name text;
ALTER TABLE queue.tickets ADD CONSTRAINT ticket_service_pair CHECK ((service_id IS NULL)=(service_name IS NULL));
ALTER TABLE queue.tickets ADD CONSTRAINT ticket_service_link FOREIGN KEY (tenant_id,queue_id,service_id)
    REFERENCES queue.service_links (tenant_id,queue_id,service_id);
CREATE OR REPLACE VIEW queue.ticket_details WITH (security_invoker=true) AS
SELECT tenant_id,queue_id,id,position,to_char(service_date,'YYYY-MM-DD') AS service_date,number,status,called_by,
    to_char(issued_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS issued_at,
    to_char(called_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS called_at,
    to_char(closed_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS closed_at,service_id,service_name
FROM queue.tickets;

CREATE FUNCTION queue.allocate_ticket(p_tenant uuid,p_queue uuid,p_service uuid)
RETURNS uuid LANGUAGE plpgsql SECURITY INVOKER SET search_path=pg_catalog AS $function$
DECLARE v_position bigint; v_number integer; v_zone text; v_at timestamptz; v_day date;
    v_id uuid:=gen_random_uuid(); v_service_name text;
BEGIN
    PERFORM 1 FROM queue.queues WHERE tenant_id=p_tenant AND id=p_queue FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'queue not found' USING ERRCODE='Q2001'; END IF;
    IF p_service IS NOT NULL THEN
        SELECT s.name INTO v_service_name FROM catalog.services s JOIN queue.service_links link
            ON link.tenant_id=s.tenant_id AND link.service_id=s.id
            WHERE link.tenant_id=p_tenant AND link.queue_id=p_queue AND s.id=p_service AND s.enabled;
        IF NOT FOUND THEN RAISE EXCEPTION 'service unavailable' USING ERRCODE='Q2010'; END IF;
    END IF;
    SELECT l.time_zone INTO v_zone FROM queue.queues q JOIN core.locations l
        ON l.tenant_id=q.tenant_id AND l.id=q.location_id WHERE q.tenant_id=p_tenant AND q.id=p_queue;
    v_at:=clock_timestamp(); v_day:=(v_at AT TIME ZONE v_zone)::date;
    UPDATE queue.queues SET next_position=next_position+1 WHERE tenant_id=p_tenant AND id=p_queue RETURNING next_position INTO v_position;
    INSERT INTO queue.daily_counters (tenant_id,queue_id,service_date,last_number) VALUES (p_tenant,p_queue,v_day,1)
        ON CONFLICT (tenant_id,queue_id,service_date) DO UPDATE SET last_number=queue.daily_counters.last_number+1
        RETURNING last_number INTO v_number;
    INSERT INTO queue.tickets (tenant_id,queue_id,id,position,service_date,number,issued_at,service_id,service_name)
        VALUES (p_tenant,p_queue,v_id,v_position,v_day,v_number,v_at,p_service,v_service_name);
    RETURN v_id;
END
$function$;
REVOKE ALL ON FUNCTION queue.allocate_ticket(uuid,uuid,uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION queue.allocate_ticket(uuid,uuid,uuid) TO turnesta_runtime;
CREATE OR REPLACE FUNCTION queue.allocate_ticket(p_tenant uuid,p_queue uuid)
RETURNS uuid LANGUAGE sql SECURITY INVOKER SET search_path=pg_catalog AS $function$
    SELECT queue.allocate_ticket(p_tenant,p_queue,NULL)
$function$;

CREATE FUNCTION queue.public_services(p_slug uuid,p_after uuid,p_limit integer)
RETURNS TABLE (id uuid,name text,description text) LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $function$
    SELECT s.id,s.name,s.description FROM queue.public_settings pub
    JOIN queue.service_links link ON link.tenant_id=pub.tenant_id AND link.queue_id=pub.queue_id
    JOIN catalog.services s ON s.tenant_id=link.tenant_id AND s.id=link.service_id
    WHERE pub.public_slug=p_slug AND s.enabled AND (p_after IS NULL OR s.id>p_after)
    ORDER BY s.id LIMIT greatest(1,least(p_limit,100))+1
$function$;
REVOKE ALL ON FUNCTION queue.public_services(uuid,uuid,integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION queue.public_services(uuid,uuid,integer) TO turnesta_runtime;

CREATE FUNCTION queue.public_ticket_with_service(p_slug uuid,p_hash bytea)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $function$
DECLARE v_ticket jsonb; v_name text;
BEGIN
    v_ticket:=queue.public_ticket_with_tips(p_slug,p_hash);
    SELECT t.service_name INTO v_name FROM queue.public_settings s JOIN queue.guest_claims c
        ON c.tenant_id=s.tenant_id AND c.queue_id=s.queue_id
        JOIN queue.tickets t ON t.tenant_id=c.tenant_id AND t.queue_id=c.queue_id AND t.id=c.ticket_id
        WHERE s.public_slug=p_slug AND c.token_hash=p_hash;
    RETURN v_ticket || jsonb_build_object('service_name',v_name);
END
$function$;
REVOKE ALL ON FUNCTION queue.public_ticket_with_service(uuid,bytea) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION queue.public_ticket_with_service(uuid,bytea) TO turnesta_runtime;

CREATE FUNCTION queue.public_issue(p_slug uuid,p_hash bytea,p_service uuid)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $function$
DECLARE v_tenant uuid; v_queue uuid; v_open boolean; v_ticket uuid; v_count bigint; v_now timestamptz;
BEGIN
    IF octet_length(p_hash) IS DISTINCT FROM 32 THEN RAISE EXCEPTION 'invalid token' USING ERRCODE='Q2002'; END IF;
    SELECT s.tenant_id,s.queue_id INTO v_tenant,v_queue FROM queue.public_settings s WHERE s.public_slug=p_slug;
    IF NOT FOUND THEN RAISE EXCEPTION 'queue not found' USING ERRCODE='Q2001'; END IF;
    PERFORM 1 FROM queue.queues WHERE tenant_id=v_tenant AND id=v_queue FOR UPDATE;
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
REVOKE ALL ON FUNCTION queue.public_issue(uuid,bytea,uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION queue.public_issue(uuid,bytea,uuid) TO turnesta_runtime;
