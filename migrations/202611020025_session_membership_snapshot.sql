-- Authenticate the session and its requested tenant membership in one snapshot.
-- An in-flight request from an unverified account cannot acquire a newly granted
-- tenant after mailbox verification has revoked that account's old sessions.
CREATE FUNCTION auth.find_scoped_session(p_token_hash bytea,p_tenant uuid)
RETURNS TABLE(user_id uuid,csrf_hash bytea,tenant_member boolean)
LANGUAGE sql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
    SELECT session.user_id,session.csrf_hash,
        p_tenant IS NULL OR EXISTS(SELECT 1 FROM core.memberships member
            WHERE member.user_id=session.user_id AND member.tenant_id=p_tenant AND member.active)
    FROM auth.sessions session
    WHERE session.token_hash=p_token_hash AND session.revoked_at IS NULL
        AND session.expires_at>clock_timestamp()
$f$;
REVOKE ALL ON FUNCTION auth.find_scoped_session(bytea,uuid)
    FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;
GRANT EXECUTE ON FUNCTION auth.find_scoped_session(bytea,uuid) TO turnesta_runtime;
