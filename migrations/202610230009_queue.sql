CREATE SCHEMA queue;
REVOKE ALL ON SCHEMA queue FROM PUBLIC;
GRANT USAGE ON SCHEMA queue TO turnesta_runtime;

CREATE TABLE queue.queues (
    tenant_id uuid NOT NULL,
    id uuid NOT NULL,
    location_id uuid NOT NULL,
    name text NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 120 AND name = btrim(name)
        AND name !~ '[[:cntrl:]]'
    ),
    version bigint NOT NULL DEFAULT 1 CHECK (version > 0),
    next_position bigint NOT NULL DEFAULT 0 CHECK (next_position >= 0),
    PRIMARY KEY (tenant_id, id),
    FOREIGN KEY (tenant_id, location_id) REFERENCES core.locations (tenant_id, id)
);
CREATE INDEX queues_location_idx ON queue.queues (tenant_id, location_id, id);

CREATE TABLE queue.daily_counters (
    tenant_id uuid NOT NULL,
    queue_id uuid NOT NULL,
    service_date date NOT NULL,
    last_number integer NOT NULL CHECK (last_number > 0),
    PRIMARY KEY (tenant_id, queue_id, service_date),
    FOREIGN KEY (tenant_id, queue_id) REFERENCES queue.queues (tenant_id, id)
);

CREATE TABLE queue.tickets (
    tenant_id uuid NOT NULL,
    queue_id uuid NOT NULL,
    id uuid NOT NULL,
    position bigint NOT NULL CHECK (position > 0),
    service_date date NOT NULL,
    number integer NOT NULL CHECK (number > 0),
    status text NOT NULL DEFAULT 'waiting'
        CHECK (status IN ('waiting', 'called', 'completed', 'cancelled')),
    called_by uuid,
    issued_at timestamptz NOT NULL,
    called_at timestamptz,
    closed_at timestamptz,
    PRIMARY KEY (tenant_id, queue_id, id),
    UNIQUE (tenant_id, queue_id, position),
    UNIQUE (tenant_id, queue_id, service_date, number),
    FOREIGN KEY (tenant_id, queue_id) REFERENCES queue.queues (tenant_id, id),
    FOREIGN KEY (tenant_id, called_by) REFERENCES core.memberships (tenant_id, user_id),
    CHECK ((called_by IS NULL) = (called_at IS NULL)),
    CHECK (
        (status = 'waiting' AND called_at IS NULL AND closed_at IS NULL)
        OR (status = 'called' AND called_at IS NOT NULL AND closed_at IS NULL)
        OR (status = 'completed' AND called_at IS NOT NULL AND closed_at IS NOT NULL)
        OR (status = 'cancelled' AND closed_at IS NOT NULL)
    )
);
CREATE INDEX tickets_waiting_idx ON queue.tickets (tenant_id, queue_id, position)
    WHERE status = 'waiting';
CREATE INDEX tickets_status_idx ON queue.tickets (tenant_id, queue_id, status, position);
CREATE UNIQUE INDEX tickets_operator_active_idx
    ON queue.tickets (tenant_id, queue_id, called_by) WHERE status = 'called';

-- Command and result commit together. A replay returns the original snapshot,
-- including an empty CallNext, rather than inspecting the ticket's new state.
CREATE TABLE queue.commands (
    tenant_id uuid NOT NULL,
    queue_id uuid NOT NULL,
    actor_id uuid NOT NULL,
    idempotency_key uuid NOT NULL,
    command jsonb NOT NULL CHECK (jsonb_typeof(command) = 'object'),
    result jsonb NOT NULL CHECK (jsonb_typeof(result) = 'object'),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, queue_id, actor_id, idempotency_key),
    FOREIGN KEY (tenant_id, queue_id) REFERENCES queue.queues (tenant_id, id),
    FOREIGN KEY (tenant_id, actor_id) REFERENCES core.memberships (tenant_id, user_id)
);

-- SQLx needs no date/time dependency for these UTC ISO-8601 projections.
CREATE VIEW queue.ticket_details WITH (security_invoker = true) AS
SELECT tenant_id, queue_id, id, position,
    to_char(service_date, 'YYYY-MM-DD') AS service_date, number, status, called_by,
    to_char(issued_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS issued_at,
    to_char(called_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS called_at,
    to_char(closed_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS closed_at
FROM queue.tickets;

REVOKE ALL ON ALL TABLES IN SCHEMA queue FROM PUBLIC;
GRANT SELECT ON ALL TABLES IN SCHEMA queue TO turnesta_runtime;
GRANT INSERT ON queue.queues, queue.commands TO turnesta_runtime;
GRANT UPDATE (version, next_position) ON queue.queues TO turnesta_runtime;
GRANT INSERT, UPDATE ON queue.daily_counters, queue.tickets TO turnesta_runtime;

DO $policies$
DECLARE target text;
BEGIN
    FOREACH target IN ARRAY ARRAY['queues', 'daily_counters', 'tickets', 'commands'] LOOP
        EXECUTE format('ALTER TABLE queue.%I ENABLE ROW LEVEL SECURITY', target);
        EXECUTE format('ALTER TABLE queue.%I FORCE ROW LEVEL SECURITY', target);
        EXECUTE format($sql$
            CREATE POLICY tenant_read ON queue.%I FOR SELECT TO turnesta_runtime
            USING (
                tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
                AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = %1$I.tenant_id)
            )
        $sql$, target);
    END LOOP;
    FOREACH target IN ARRAY ARRAY['daily_counters', 'tickets'] LOOP
        EXECUTE format($sql$
            CREATE POLICY operator_insert ON queue.%I FOR INSERT TO turnesta_runtime
            WITH CHECK (
                tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
                AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = %1$I.tenant_id
                    AND m.role IN ('owner', 'manager', 'operator'))
            )
        $sql$, target);
    END LOOP;
    FOREACH target IN ARRAY ARRAY['queues', 'daily_counters', 'tickets'] LOOP
        EXECUTE format($sql$
            CREATE POLICY operator_update ON queue.%I FOR UPDATE TO turnesta_runtime
            USING (
                tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
                AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = %1$I.tenant_id
                    AND m.role IN ('owner', 'manager', 'operator'))
            ) WITH CHECK (
                tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
                AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = %1$I.tenant_id
                    AND m.role IN ('owner', 'manager', 'operator'))
            )
        $sql$, target);
    END LOOP;
END
$policies$;

CREATE POLICY manager_insert ON queue.queues FOR INSERT TO turnesta_runtime
WITH CHECK (
    tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
    AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = queues.tenant_id
        AND m.role IN ('owner', 'manager'))
);
CREATE POLICY actor_read ON queue.commands AS RESTRICTIVE FOR SELECT TO turnesta_runtime
    USING (actor_id = nullif(current_setting('app.user_id', true), '')::uuid);
CREATE POLICY actor_insert ON queue.commands FOR INSERT TO turnesta_runtime
WITH CHECK (
    tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
    AND actor_id = nullif(current_setting('app.user_id', true), '')::uuid
    AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = commands.tenant_id
        AND m.role IN ('owner', 'manager', 'operator'))
);
