CREATE INDEX invitation_deliveries_oldest_unsent_idx
    ON auth.invitation_deliveries (created_at) WHERE state IN ('pending', 'leased');

CREATE FUNCTION auth.oldest_unsent_email_age_seconds() RETURNS double precision
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog
AS $function$
    SELECT GREATEST(0::double precision,
        EXTRACT(EPOCH FROM (statement_timestamp() - min(created_at)))::double precision)
    FROM auth.invitation_deliveries
    WHERE state IN ('pending', 'leased')
$function$;
REVOKE ALL ON FUNCTION auth.oldest_unsent_email_age_seconds() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION auth.oldest_unsent_email_age_seconds() TO turnesta_delivery_worker;
