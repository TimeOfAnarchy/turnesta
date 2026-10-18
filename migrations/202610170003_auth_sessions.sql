CREATE SCHEMA auth;
REVOKE ALL ON SCHEMA auth FROM PUBLIC, turnesta_runtime, turnesta_provisioner;
GRANT USAGE ON SCHEMA auth TO turnesta_runtime;

-- Server-side sessions are global identities, not tenant authorizations. Only
-- digests enter this table; the HTTP layer owns raw cookie/CSRF token handling.
CREATE TABLE auth.sessions (
    token_hash bytea PRIMARY KEY CHECK (octet_length(token_hash) = 32),
    user_id uuid NOT NULL REFERENCES core.users (id) ON DELETE CASCADE,
    csrf_hash bytea NOT NULL CHECK (octet_length(csrf_hash) = 32),
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL CHECK (expires_at > created_at),
    revoked_at timestamptz
);
CREATE INDEX sessions_expires_at_idx ON auth.sessions (expires_at);
CREATE INDEX sessions_user_id_idx ON auth.sessions (user_id);
REVOKE ALL ON auth.sessions FROM PUBLIC, turnesta_runtime, turnesta_provisioner;

-- The runtime may issue a session only after the application has verified the
-- identity through its login method. This function itself is not a login flow.
CREATE FUNCTION auth.create_session(
    p_user_id uuid,
    p_token_hash bytea,
    p_csrf_hash bytea,
    p_ttl_seconds integer
)
RETURNS void
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $function$
DECLARE
    issued_at pg_catalog.timestamptz := pg_catalog.clock_timestamp();
BEGIN
    IF p_user_id IS NULL
        OR p_token_hash IS NULL OR pg_catalog.octet_length(p_token_hash) <> 32
        OR p_csrf_hash IS NULL OR pg_catalog.octet_length(p_csrf_hash) <> 32
        OR p_ttl_seconds IS NULL OR p_ttl_seconds NOT BETWEEN 60 AND 86400
    THEN
        RAISE EXCEPTION 'session requires a user, two 32-byte hashes and a TTL of 60..86400 seconds'
            USING ERRCODE = '22023';
    END IF;

    INSERT INTO auth.sessions (token_hash, user_id, csrf_hash, created_at, expires_at)
    VALUES (
        p_token_hash, p_user_id, p_csrf_hash, issued_at,
        issued_at + pg_catalog.make_interval(secs => p_ttl_seconds)
    );
END
$function$;

CREATE FUNCTION auth.find_session(p_token_hash bytea)
RETURNS TABLE (user_id uuid, csrf_hash bytea)
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
ROWS 1
AS $function$
BEGIN
    IF p_token_hash IS NULL OR pg_catalog.octet_length(p_token_hash) <> 32 THEN
        RAISE EXCEPTION 'session hash must contain 32 bytes' USING ERRCODE = '22023';
    END IF;

    RETURN QUERY
    SELECT session.user_id, session.csrf_hash
    FROM auth.sessions AS session
    WHERE session.token_hash = p_token_hash
        AND session.revoked_at IS NULL
        AND session.expires_at > pg_catalog.clock_timestamp();
END
$function$;

CREATE FUNCTION auth.revoke_session(p_token_hash bytea)
RETURNS void
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $function$
BEGIN
    IF p_token_hash IS NULL OR pg_catalog.octet_length(p_token_hash) <> 32 THEN
        RAISE EXCEPTION 'session hash must contain 32 bytes' USING ERRCODE = '22023';
    END IF;

    UPDATE auth.sessions AS session
    SET revoked_at = pg_catalog.clock_timestamp()
    WHERE session.token_hash = p_token_hash AND session.revoked_at IS NULL;
END
$function$;

-- PostgreSQL grants new functions' EXECUTE to PUBLIC unless explicitly revoked.
-- SQLx applies this migration transactionally, so no public grant is committed.
REVOKE ALL ON FUNCTION auth.create_session(uuid, bytea, bytea, integer)
    FROM PUBLIC, turnesta_runtime, turnesta_provisioner;
REVOKE ALL ON FUNCTION auth.find_session(bytea)
    FROM PUBLIC, turnesta_runtime, turnesta_provisioner;
REVOKE ALL ON FUNCTION auth.revoke_session(bytea)
    FROM PUBLIC, turnesta_runtime, turnesta_provisioner;
GRANT EXECUTE ON FUNCTION auth.create_session(uuid, bytea, bytea, integer) TO turnesta_runtime;
GRANT EXECUTE ON FUNCTION auth.find_session(bytea) TO turnesta_runtime;
GRANT EXECUTE ON FUNCTION auth.revoke_session(bytea) TO turnesta_runtime;
