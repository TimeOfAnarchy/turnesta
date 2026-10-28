ALTER TABLE queue.public_settings ADD COLUMN display_enabled boolean NOT NULL DEFAULT false;

-- Each page is one MVCC snapshot. Clients assemble pages only when every
-- response has the same queue version; no internal ticket/operator IDs leave DB.
CREATE FUNCTION queue.public_display(p_slug uuid, p_page integer)
RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $function$
    SELECT jsonb_build_object(
        'queue_name', q.name, 'location_name', l.name, 'organization_name', o.name,
        'time_zone', l.time_zone, 'version', q.version::text,
        'observed_at', to_char(statement_timestamp() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'),
        'join_open', s.join_open, 'waiting_count', totals.waiting_count,
        'called_count', totals.called_count, 'page', p_page,
        'next_page', CASE WHEN totals.called_count > (p_page::bigint + 1) * 100 THEN p_page + 1 ELSE NULL END,
        'called', coalesce((SELECT jsonb_agg(jsonb_build_object(
            'number', t.number, 'service_date', to_char(t.service_date, 'YYYY-MM-DD')) ORDER BY t.position)
            FROM (SELECT number, service_date, position FROM queue.tickets
                WHERE tenant_id = q.tenant_id AND queue_id = q.id AND status = 'called'
                ORDER BY position LIMIT 100 OFFSET p_page::bigint * 100) t), '[]'::jsonb)
    )
    FROM queue.public_settings s
    JOIN queue.queues q ON q.tenant_id = s.tenant_id AND q.id = s.queue_id
    JOIN core.locations l ON l.tenant_id = q.tenant_id AND l.id = q.location_id
    JOIN core.organizations o ON o.id = q.tenant_id
    CROSS JOIN LATERAL (SELECT count(*) FILTER (WHERE status = 'waiting') AS waiting_count,
        count(*) FILTER (WHERE status = 'called') AS called_count FROM queue.tickets
        WHERE tenant_id = q.tenant_id AND queue_id = q.id AND status IN ('waiting', 'called')) totals
    WHERE s.public_slug = p_slug AND s.display_enabled AND p_page BETWEEN 0 AND 21474835
$function$;
REVOKE ALL ON FUNCTION queue.public_display(uuid, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION queue.public_display(uuid, integer) TO turnesta_runtime;
