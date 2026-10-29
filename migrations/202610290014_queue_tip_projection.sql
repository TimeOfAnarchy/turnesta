-- Validate the capability before looking up optional settings. A SQL join can
-- otherwise skip the validating function for an unknown slug and return NULL.
CREATE OR REPLACE FUNCTION queue.public_ticket_with_tips(p_slug uuid, p_hash bytea)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $function$
DECLARE v_ticket jsonb; v_tip jsonb;
BEGIN
    SELECT to_jsonb(ticket) INTO STRICT v_ticket FROM queue.public_ticket(p_slug,p_hash) ticket;
    IF v_ticket->>'status' = 'completed' THEN
        SELECT jsonb_build_object('public_slug',r.public_slug,'display_name',r.display_name,'environment',r.environment)
        INTO v_tip FROM queue.public_settings s
        JOIN queue.tip_settings ts ON ts.tenant_id=s.tenant_id AND ts.queue_id=s.queue_id
        JOIN tips.recipients r ON r.tenant_id=ts.tenant_id AND r.id=ts.recipient_id AND r.location_id=ts.location_id
        WHERE s.public_slug=p_slug AND r.environment='mock';
    END IF;
    RETURN v_ticket || jsonb_build_object('tips',v_tip);
END
$function$;
