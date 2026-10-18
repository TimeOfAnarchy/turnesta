GRANT USAGE ON SCHEMA auth TO turnesta_provisioner;

-- Identity bindings are independent of tenant membership and remain attached
-- to the same Core user when another login provider is added later.
CREATE TABLE auth.identities (
    id uuid PRIMARY KEY DEFAULT pg_catalog.gen_random_uuid(),
    provider text NOT NULL CHECK (provider = 'email'),
    subject text NOT NULL CHECK (
        char_length(subject) BETWEEN 3 AND 254
        AND octet_length(subject) = char_length(subject)
        AND subject = lower(subject)
        AND subject = btrim(subject)
        AND subject !~ '[[:space:][:cntrl:]]'
        AND subject ~ '^[^@]+@[^@]+$'
    ),
    user_id uuid NOT NULL REFERENCES core.users (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT pg_catalog.clock_timestamp(),
    verified_at timestamptz,
    UNIQUE (provider, subject),
    UNIQUE (user_id, provider)
);

CREATE TABLE auth.login_codes (
    token_hash bytea PRIMARY KEY CHECK (octet_length(token_hash) = 32),
    identity_id uuid NOT NULL REFERENCES auth.identities (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL CHECK (expires_at > created_at),
    used_at timestamptz
);
CREATE INDEX login_codes_identity_active_idx ON auth.login_codes (identity_id) WHERE used_at IS NULL;
CREATE INDEX login_codes_expires_at_idx ON auth.login_codes (expires_at);
REVOKE ALL ON auth.identities, auth.login_codes FROM PUBLIC, turnesta_runtime, turnesta_provisioner;

CREATE FUNCTION auth.bind_email(p_user_id uuid, p_email text)
RETURNS void
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $function$
DECLARE
    bound_user pg_catalog.uuid;
BEGIN
    IF p_user_id IS NULL OR p_email IS NULL THEN
        RAISE EXCEPTION 'email binding requires a user and a normalized email' USING ERRCODE = '22023';
    END IF;

    -- Unique constraints serialize conflicting bindings. A repeated identical
    -- binding preserves identity ID and verification; reassignment is refused.
    INSERT INTO auth.identities (provider, subject, user_id)
    VALUES ('email', p_email, p_user_id)
    ON CONFLICT DO NOTHING;

    SELECT identity.user_id INTO bound_user
    FROM auth.identities AS identity
    WHERE identity.provider = 'email' AND identity.subject = p_email;

    IF bound_user IS DISTINCT FROM p_user_id THEN
        RAISE EXCEPTION 'email identity binding conflicts with an existing binding' USING ERRCODE = '23505';
    END IF;
END
$function$;

CREATE FUNCTION auth.issue_login_code(p_email text, p_token_hash bytea, p_ttl_seconds integer)
RETURNS void
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $function$
DECLARE
    bound_identity pg_catalog.uuid;
    issued_at pg_catalog.timestamptz;
BEGIN
    IF p_email IS NULL OR p_token_hash IS NULL OR pg_catalog.octet_length(p_token_hash) <> 32
        OR p_ttl_seconds IS NULL OR p_ttl_seconds NOT BETWEEN 60 AND 600
    THEN
        RAISE EXCEPTION 'login code requires an email, a 32-byte hash and a TTL of 60..600 seconds'
            USING ERRCODE = '22023';
    END IF;

    SELECT identity.id INTO bound_identity
    FROM auth.identities AS identity
    WHERE identity.provider = 'email' AND identity.subject = p_email
    FOR UPDATE;
    IF bound_identity IS NULL THEN
        RAISE EXCEPTION 'email identity does not exist' USING ERRCODE = '22023';
    END IF;

    -- Issuance, consumption and invalidation lock the identity before its code
    -- rows, giving one consistent order and at most one unused issued code.
    issued_at := pg_catalog.clock_timestamp();
    UPDATE auth.login_codes AS code
    SET used_at = issued_at
    WHERE code.identity_id = bound_identity AND code.used_at IS NULL;

    INSERT INTO auth.login_codes (token_hash, identity_id, created_at, expires_at)
    VALUES (p_token_hash, bound_identity, issued_at, issued_at + pg_catalog.make_interval(secs => p_ttl_seconds));
END
$function$;

CREATE FUNCTION auth.invalidate_login_code(p_token_hash bytea)
RETURNS void
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $function$
DECLARE
    bound_identity pg_catalog.uuid;
BEGIN
    IF p_token_hash IS NULL OR pg_catalog.octet_length(p_token_hash) <> 32 THEN
        RAISE EXCEPTION 'login code hash must contain 32 bytes' USING ERRCODE = '22023';
    END IF;

    SELECT code.identity_id INTO bound_identity FROM auth.login_codes AS code WHERE code.token_hash = p_token_hash;
    IF bound_identity IS NULL THEN
        RETURN;
    END IF;
    PERFORM 1 FROM auth.identities AS identity WHERE identity.id = bound_identity FOR UPDATE;
    UPDATE auth.login_codes AS code
    SET used_at = pg_catalog.clock_timestamp()
    WHERE code.token_hash = p_token_hash AND code.used_at IS NULL;
END
$function$;

CREATE FUNCTION auth.consume_login_code(
    p_code_hash bytea,
    p_session_hash bytea,
    p_csrf_hash bytea,
    p_session_ttl_seconds integer,
    p_previous_session_hash bytea DEFAULT NULL
)
RETURNS uuid
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $function$
DECLARE
    bound_identity pg_catalog.uuid;
    bound_user pg_catalog.uuid;
    consumed_identity pg_catalog.uuid;
BEGIN
    IF p_code_hash IS NULL OR pg_catalog.octet_length(p_code_hash) <> 32
        OR p_session_hash IS NULL OR pg_catalog.octet_length(p_session_hash) <> 32
        OR p_csrf_hash IS NULL OR pg_catalog.octet_length(p_csrf_hash) <> 32
        OR p_session_ttl_seconds IS NULL OR p_session_ttl_seconds NOT BETWEEN 60 AND 86400
        OR (p_previous_session_hash IS NOT NULL AND pg_catalog.octet_length(p_previous_session_hash) <> 32)
    THEN
        RAISE EXCEPTION 'login requires 32-byte hashes and a session TTL of 60..86400 seconds'
            USING ERRCODE = '22023';
    END IF;

    SELECT code.identity_id INTO bound_identity FROM auth.login_codes AS code WHERE code.token_hash = p_code_hash;
    IF bound_identity IS NULL THEN
        RETURN NULL;
    END IF;

    SELECT identity.user_id INTO bound_user
    FROM auth.identities AS identity WHERE identity.id = bound_identity FOR UPDATE;
    IF bound_user IS NULL THEN
        RETURN NULL;
    END IF;

    UPDATE auth.login_codes AS code
    SET used_at = pg_catalog.clock_timestamp()
    WHERE code.token_hash = p_code_hash AND code.used_at IS NULL
        AND code.expires_at > pg_catalog.clock_timestamp()
    RETURNING code.identity_id INTO consumed_identity;
    IF consumed_identity IS NULL THEN
        RETURN NULL;
    END IF;

    UPDATE auth.identities AS identity
    SET verified_at = COALESCE(identity.verified_at, pg_catalog.clock_timestamp())
    WHERE identity.id = bound_identity;
    PERFORM auth.create_session(bound_user, p_session_hash, p_csrf_hash, p_session_ttl_seconds);
    IF p_previous_session_hash IS NOT NULL THEN
        PERFORM auth.revoke_session(p_previous_session_hash);
    END IF;
    RETURN bound_user;
END
$function$;

REVOKE ALL ON FUNCTION auth.bind_email(uuid, text) FROM PUBLIC, turnesta_runtime, turnesta_provisioner;
REVOKE ALL ON FUNCTION auth.issue_login_code(text, bytea, integer) FROM PUBLIC, turnesta_runtime, turnesta_provisioner;
REVOKE ALL ON FUNCTION auth.invalidate_login_code(bytea) FROM PUBLIC, turnesta_runtime, turnesta_provisioner;
REVOKE ALL ON FUNCTION auth.consume_login_code(bytea, bytea, bytea, integer, bytea)
    FROM PUBLIC, turnesta_runtime, turnesta_provisioner;
GRANT EXECUTE ON FUNCTION auth.bind_email(uuid, text) TO turnesta_provisioner;
GRANT EXECUTE ON FUNCTION auth.issue_login_code(text, bytea, integer) TO turnesta_provisioner;
GRANT EXECUTE ON FUNCTION auth.invalidate_login_code(bytea) TO turnesta_provisioner;
GRANT EXECUTE ON FUNCTION auth.consume_login_code(bytea, bytea, bytea, integer, bytea) TO turnesta_runtime;
