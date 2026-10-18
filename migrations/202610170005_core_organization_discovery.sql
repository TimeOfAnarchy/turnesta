-- Narrow cross-tenant projection for the already authenticated user. Normal
-- tenant policies stay unchanged. The server supplies the session's user ID.
CREATE FUNCTION core.list_user_organizations(p_user_id uuid, p_after_id uuid, p_limit integer)
RETURNS TABLE (id uuid, name text, role text)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog
AS $function$
    SELECT organization.id, organization.name, membership.role
    FROM core.organizations AS organization
    JOIN core.memberships AS membership ON membership.tenant_id = organization.id
    WHERE membership.user_id = p_user_id
        AND (p_after_id IS NULL OR organization.id > p_after_id)
    ORDER BY organization.id
    LIMIT least(greatest(p_limit, 1), 100) + 1
$function$;
REVOKE ALL ON FUNCTION core.list_user_organizations(uuid, uuid, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION core.list_user_organizations(uuid, uuid, integer) TO turnesta_runtime;
