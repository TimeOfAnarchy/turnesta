CREATE TABLE queue.public_settings (
    tenant_id uuid NOT NULL,
    queue_id uuid NOT NULL,
    public_slug uuid NOT NULL UNIQUE,
    join_open boolean NOT NULL,
    version integer NOT NULL DEFAULT 1 CHECK (version > 0),
    PRIMARY KEY (tenant_id, queue_id),
    FOREIGN KEY (tenant_id, queue_id) REFERENCES queue.queues (tenant_id, id)
);
REVOKE ALL ON queue.public_settings FROM PUBLIC;
GRANT SELECT, INSERT, UPDATE ON queue.public_settings TO turnesta_runtime;
ALTER TABLE queue.public_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE queue.public_settings FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_read ON queue.public_settings FOR SELECT TO turnesta_runtime USING (
    tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
    AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = public_settings.tenant_id)
);
CREATE POLICY manager_insert ON queue.public_settings FOR INSERT TO turnesta_runtime WITH CHECK (
    tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
    AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = public_settings.tenant_id
        AND m.role IN ('owner', 'manager'))
);
CREATE POLICY manager_update ON queue.public_settings FOR UPDATE TO turnesta_runtime USING (
    tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
    AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = public_settings.tenant_id
        AND m.role IN ('owner', 'manager'))
) WITH CHECK (
    tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
    AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = public_settings.tenant_id
        AND m.role IN ('owner', 'manager'))
);

CREATE TABLE queue.guest_claims (
    tenant_id uuid NOT NULL,
    queue_id uuid NOT NULL,
    token_hash bytea NOT NULL CHECK (octet_length(token_hash) = 32),
    ticket_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, queue_id, token_hash),
    UNIQUE (tenant_id, queue_id, ticket_id),
    FOREIGN KEY (tenant_id, queue_id) REFERENCES queue.public_settings (tenant_id, queue_id),
    FOREIGN KEY (tenant_id, queue_id, ticket_id) REFERENCES queue.tickets (tenant_id, queue_id, id)
);
CREATE INDEX guest_claims_budget_idx ON queue.guest_claims (tenant_id, queue_id, created_at);
REVOKE ALL ON queue.guest_claims FROM PUBLIC, turnesta_runtime, turnesta_provisioner;
ALTER TABLE queue.guest_claims ENABLE ROW LEVEL SECURITY;
ALTER TABLE queue.guest_claims FORCE ROW LEVEL SECURITY;

-- The function owner inherits the provisioning capability. No raw guest-table
-- privileges are granted to runtime or provisioner logins by these policies.
DO $policies$
DECLARE target text;
BEGIN
    FOREACH target IN ARRAY ARRAY['queues', 'tickets', 'daily_counters', 'public_settings', 'guest_claims'] LOOP
        EXECUTE format('CREATE POLICY provisioner ON queue.%I TO turnesta_provisioner USING (true) WITH CHECK (true)', target);
    END LOOP;
END
$policies$;

-- One allocator for staff and public issuance. SECURITY INVOKER preserves the
-- caller's existing grants/RLS; it does not grant anonymous access by itself.
CREATE FUNCTION queue.allocate_ticket(p_tenant uuid, p_queue uuid)
RETURNS uuid LANGUAGE plpgsql SECURITY INVOKER SET search_path = pg_catalog AS $function$
DECLARE
    v_position bigint;
    v_number integer;
    v_zone text;
    v_at timestamptz;
    v_day date;
    v_id uuid := gen_random_uuid();
BEGIN
    PERFORM 1 FROM queue.queues WHERE tenant_id = p_tenant AND id = p_queue FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'queue not found' USING ERRCODE = 'Q2001'; END IF;
    SELECT l.time_zone INTO v_zone FROM queue.queues q JOIN core.locations l
        ON l.tenant_id = q.tenant_id AND l.id = q.location_id
        WHERE q.tenant_id = p_tenant AND q.id = p_queue;
    v_at := clock_timestamp();
    v_day := (v_at AT TIME ZONE v_zone)::date;
    UPDATE queue.queues SET next_position = next_position + 1
        WHERE tenant_id = p_tenant AND id = p_queue RETURNING next_position INTO v_position;
    INSERT INTO queue.daily_counters (tenant_id, queue_id, service_date, last_number)
        VALUES (p_tenant, p_queue, v_day, 1)
        ON CONFLICT (tenant_id, queue_id, service_date)
        DO UPDATE SET last_number = queue.daily_counters.last_number + 1
        RETURNING last_number INTO v_number;
    INSERT INTO queue.tickets (tenant_id, queue_id, id, position, service_date, number, issued_at)
        VALUES (p_tenant, p_queue, v_id, v_position, v_day, v_number, v_at);
    RETURN v_id;
END
$function$;
REVOKE ALL ON FUNCTION queue.allocate_ticket(uuid, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION queue.allocate_ticket(uuid, uuid) TO turnesta_runtime;

CREATE FUNCTION queue.public_profile(p_slug uuid)
RETURNS TABLE (public_slug uuid, queue_name text, location_name text, organization_name text, time_zone text, join_open boolean)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $function$
    SELECT s.public_slug, q.name, l.name, o.name, l.time_zone, s.join_open
    FROM queue.public_settings s JOIN queue.queues q ON q.tenant_id = s.tenant_id AND q.id = s.queue_id
    JOIN core.locations l ON l.tenant_id = q.tenant_id AND l.id = q.location_id
    JOIN core.organizations o ON o.id = q.tenant_id WHERE s.public_slug = p_slug
$function$;
REVOKE ALL ON FUNCTION queue.public_profile(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION queue.public_profile(uuid) TO turnesta_runtime;

CREATE FUNCTION queue.public_ticket(p_slug uuid, p_hash bytea)
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
    IF NOT FOUND THEN RAISE EXCEPTION 'ticket not found' USING ERRCODE = 'Q2001'; END IF;
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
REVOKE ALL ON FUNCTION queue.public_ticket(uuid, bytea) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION queue.public_ticket(uuid, bytea) TO turnesta_runtime;

CREATE FUNCTION queue.public_issue(p_slug uuid, p_hash bytea)
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
REVOKE ALL ON FUNCTION queue.public_issue(uuid, bytea) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION queue.public_issue(uuid, bytea) TO turnesta_runtime;
