GRANT USAGE ON SCHEMA auth TO turnesta_delivery_worker;

CREATE TABLE auth.invitation_deliveries (
    id uuid PRIMARY KEY,
    identity_id uuid NOT NULL REFERENCES auth.identities (id) ON DELETE CASCADE,
    code_hash bytea NOT NULL UNIQUE REFERENCES auth.login_codes (token_hash) ON DELETE CASCADE,
    key_id bytea NOT NULL CHECK (octet_length(key_id) = 32),
    nonce bytea CHECK (octet_length(nonce) = 24),
    ciphertext bytea CHECK (octet_length(ciphertext) BETWEEN 16 AND 8192),
    state text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'leased', 'sent', 'failed', 'expired', 'canceled')),
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 5),
    available_at timestamptz NOT NULL DEFAULT pg_catalog.clock_timestamp(),
    lease_token uuid,
    lease_until timestamptz,
    created_at timestamptz NOT NULL DEFAULT pg_catalog.clock_timestamp(),
    expires_at timestamptz NOT NULL,
    finished_at timestamptz,
    last_error_category text CHECK (last_error_category IN ('smtp_failure', 'invalid_payload', 'superseded', 'code_used', 'expired', 'attempts_exhausted')),
    CHECK (expires_at > created_at),
    CHECK (
        (state = 'leased' AND lease_token IS NOT NULL AND lease_until IS NOT NULL)
        OR (state <> 'leased' AND lease_token IS NULL AND lease_until IS NULL)
    ),
    CHECK (
        (state IN ('pending', 'leased') AND nonce IS NOT NULL AND ciphertext IS NOT NULL AND finished_at IS NULL)
        OR (state NOT IN ('pending', 'leased') AND nonce IS NULL AND ciphertext IS NULL AND finished_at IS NOT NULL)
    )
);
CREATE INDEX invitation_deliveries_ready_idx ON auth.invitation_deliveries (key_id, available_at, created_at)
    WHERE state IN ('pending', 'leased');
CREATE INDEX invitation_deliveries_identity_active_idx ON auth.invitation_deliveries (identity_id)
    WHERE state IN ('pending', 'leased');
CREATE INDEX invitation_deliveries_expiry_idx ON auth.invitation_deliveries (expires_at)
    WHERE state IN ('pending', 'leased');
REVOKE ALL ON auth.invitation_deliveries FROM PUBLIC, turnesta_runtime, turnesta_provisioner, turnesta_delivery_worker;

CREATE FUNCTION auth.enqueue_invitation(
    p_job_id uuid, p_user_id uuid, p_email text, p_code_hash bytea,
    p_key_id bytea, p_nonce bytea, p_ciphertext bytea
)
RETURNS void
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $function$
DECLARE
    bound_identity pg_catalog.uuid;
    code_expiry pg_catalog.timestamptz;
BEGIN
    IF p_job_id IS NULL OR p_user_id IS NULL OR p_email IS NULL
        OR p_code_hash IS NULL OR pg_catalog.octet_length(p_code_hash) <> 32
        OR p_key_id IS NULL OR pg_catalog.octet_length(p_key_id) <> 32
        OR p_nonce IS NULL OR pg_catalog.octet_length(p_nonce) <> 24
        OR p_ciphertext IS NULL OR pg_catalog.octet_length(p_ciphertext) NOT BETWEEN 16 AND 8192
    THEN
        RAISE EXCEPTION 'invalid invitation delivery parameters' USING ERRCODE = '22023';
    END IF;

    -- Binding, superseding the previous code and durable enqueue commit together.
    PERFORM auth.bind_email(p_user_id, p_email);
    PERFORM auth.issue_login_code(p_email, p_code_hash, 600);
    SELECT code.identity_id, code.expires_at INTO bound_identity, code_expiry
    FROM auth.login_codes AS code WHERE code.token_hash = p_code_hash;

    UPDATE auth.invitation_deliveries AS job
    SET state = 'canceled', nonce = NULL, ciphertext = NULL, lease_token = NULL, lease_until = NULL,
        finished_at = pg_catalog.clock_timestamp(), last_error_category = 'superseded'
    WHERE job.identity_id = bound_identity AND job.state IN ('pending', 'leased');

    INSERT INTO auth.invitation_deliveries (id, identity_id, code_hash, key_id, nonce, ciphertext, expires_at)
    VALUES (p_job_id, bound_identity, p_code_hash, p_key_id, p_nonce, p_ciphertext, code_expiry);
END
$function$;

CREATE FUNCTION auth.claim_invitation_delivery(p_key_id bytea)
RETURNS TABLE (job_id uuid, lease_token uuid, code_hash bytea, nonce bytea, ciphertext bytea, attempt integer)
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
ROWS 1
AS $function$
BEGIN
    IF p_key_id IS NULL OR pg_catalog.octet_length(p_key_id) <> 32 THEN
        RAISE EXCEPTION 'delivery key identifier must contain 32 bytes' USING ERRCODE = '22023';
    END IF;

    -- Bounded housekeeping also handles a crash on the last permitted claim.
    WITH terminal AS (
        SELECT job.id, CASE
            WHEN job.expires_at <= pg_catalog.clock_timestamp() THEN 'expired'
            WHEN code.used_at IS NOT NULL THEN 'code_used'
            ELSE 'attempts_exhausted'
        END AS reason
        FROM auth.invitation_deliveries AS job
        JOIN auth.login_codes AS code ON code.token_hash = job.code_hash
        WHERE job.state IN ('pending', 'leased')
            AND (job.expires_at <= pg_catalog.clock_timestamp() OR code.used_at IS NOT NULL
                OR (job.attempts >= 5 AND (job.state = 'pending' OR job.lease_until <= pg_catalog.clock_timestamp())))
        ORDER BY job.created_at, job.id
        LIMIT 100 FOR UPDATE OF job SKIP LOCKED
    )
    UPDATE auth.invitation_deliveries AS job
    SET state = CASE terminal.reason WHEN 'expired' THEN 'expired' WHEN 'code_used' THEN 'canceled' ELSE 'failed' END,
        nonce = NULL, ciphertext = NULL, lease_token = NULL, lease_until = NULL,
        finished_at = pg_catalog.clock_timestamp(), last_error_category = terminal.reason
    FROM terminal WHERE job.id = terminal.id;

    RETURN QUERY
    WITH candidate AS (
        SELECT job.id
        FROM auth.invitation_deliveries AS job
        JOIN auth.login_codes AS code ON code.token_hash = job.code_hash
        WHERE job.key_id = p_key_id AND job.attempts < 5
            AND job.expires_at > pg_catalog.clock_timestamp() + interval '20 seconds'
            AND code.used_at IS NULL
            AND ((job.state = 'pending' AND job.available_at <= pg_catalog.clock_timestamp())
                OR (job.state = 'leased' AND job.lease_until <= pg_catalog.clock_timestamp()))
        ORDER BY job.available_at, job.created_at, job.id
        LIMIT 1 FOR UPDATE OF job SKIP LOCKED
    )
    UPDATE auth.invitation_deliveries AS job
    SET state = 'leased', attempts = job.attempts + 1,
        lease_token = pg_catalog.gen_random_uuid(), lease_until = pg_catalog.clock_timestamp() + interval '45 seconds'
    FROM candidate WHERE job.id = candidate.id
    RETURNING job.id, job.lease_token, job.code_hash, job.nonce, job.ciphertext, job.attempts;
END
$function$;

CREATE FUNCTION auth.invitation_delivery_preflight(p_job_id uuid, p_lease_token uuid)
RETURNS boolean
LANGUAGE sql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $function$
    SELECT EXISTS (
        SELECT 1 FROM auth.invitation_deliveries AS job
        JOIN auth.login_codes AS code ON code.token_hash = job.code_hash
        WHERE job.id = p_job_id AND job.state = 'leased' AND job.lease_token = p_lease_token
            AND job.lease_until > pg_catalog.clock_timestamp() + interval '20 seconds'
            AND job.expires_at > pg_catalog.clock_timestamp() + interval '20 seconds'
            AND code.used_at IS NULL
    )
$function$;

CREATE FUNCTION auth.finish_invitation_delivery(p_job_id uuid, p_lease_token uuid, p_outcome text)
RETURNS boolean
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
AS $function$
DECLARE
    claimed auth.invitation_deliveries%ROWTYPE;
    final_state pg_catalog.text;
    error_category pg_catalog.text;
BEGIN
    IF p_outcome IS NULL OR p_outcome NOT IN ('sent', 'retry', 'invalid_payload') THEN
        RAISE EXCEPTION 'invalid delivery outcome' USING ERRCODE = '22023';
    END IF;
    SELECT job.* INTO claimed FROM auth.invitation_deliveries AS job
    WHERE job.id = p_job_id AND job.state = 'leased' AND job.lease_token = p_lease_token
        AND job.lease_until > pg_catalog.clock_timestamp()
    FOR UPDATE;
    IF NOT FOUND THEN
        RETURN false;
    END IF;

    IF p_outcome = 'sent' THEN
        final_state := 'sent';
    ELSIF claimed.expires_at <= pg_catalog.clock_timestamp() THEN
        final_state := 'expired'; error_category := 'expired';
    ELSIF EXISTS (SELECT 1 FROM auth.login_codes AS code WHERE code.token_hash = claimed.code_hash AND code.used_at IS NOT NULL) THEN
        final_state := 'canceled'; error_category := 'code_used';
    ELSIF p_outcome = 'invalid_payload' THEN
        final_state := 'failed'; error_category := 'invalid_payload';
    ELSIF claimed.attempts >= 5 THEN
        final_state := 'failed'; error_category := 'attempts_exhausted';
    ELSE
        final_state := 'pending'; error_category := 'smtp_failure';
    END IF;

    UPDATE auth.invitation_deliveries AS job
    SET state = final_state, lease_token = NULL, lease_until = NULL, last_error_category = error_category,
        available_at = CASE WHEN final_state = 'pending' THEN pg_catalog.clock_timestamp() + pg_catalog.make_interval(
            secs => CASE claimed.attempts WHEN 1 THEN 15 WHEN 2 THEN 30 WHEN 3 THEN 60 ELSE 120 END
        ) ELSE job.available_at END,
        nonce = CASE WHEN final_state = 'pending' THEN job.nonce ELSE NULL END,
        ciphertext = CASE WHEN final_state = 'pending' THEN job.ciphertext ELSE NULL END,
        finished_at = CASE WHEN final_state = 'pending' THEN NULL ELSE pg_catalog.clock_timestamp() END
    WHERE job.id = p_job_id;
    RETURN true;
END
$function$;

CREATE FUNCTION auth.invitation_delivery_status(p_job_id uuid)
RETURNS TABLE (job_id uuid, state text, attempts integer, error_category text)
LANGUAGE sql
VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog
ROWS 1
AS $function$
    SELECT job.id, job.state, job.attempts, job.last_error_category
    FROM auth.invitation_deliveries AS job WHERE job.id = p_job_id
$function$;

REVOKE ALL ON FUNCTION auth.enqueue_invitation(uuid, uuid, text, bytea, bytea, bytea, bytea)
    FROM PUBLIC, turnesta_runtime, turnesta_provisioner, turnesta_delivery_worker;
REVOKE ALL ON FUNCTION auth.claim_invitation_delivery(bytea)
    FROM PUBLIC, turnesta_runtime, turnesta_provisioner, turnesta_delivery_worker;
REVOKE ALL ON FUNCTION auth.invitation_delivery_preflight(uuid, uuid)
    FROM PUBLIC, turnesta_runtime, turnesta_provisioner, turnesta_delivery_worker;
REVOKE ALL ON FUNCTION auth.finish_invitation_delivery(uuid, uuid, text)
    FROM PUBLIC, turnesta_runtime, turnesta_provisioner, turnesta_delivery_worker;
REVOKE ALL ON FUNCTION auth.invitation_delivery_status(uuid)
    FROM PUBLIC, turnesta_runtime, turnesta_provisioner, turnesta_delivery_worker;
GRANT EXECUTE ON FUNCTION auth.enqueue_invitation(uuid, uuid, text, bytea, bytea, bytea, bytea) TO turnesta_provisioner;
GRANT EXECUTE ON FUNCTION auth.invitation_delivery_status(uuid) TO turnesta_provisioner;
GRANT EXECUTE ON FUNCTION auth.claim_invitation_delivery(bytea) TO turnesta_delivery_worker;
GRANT EXECUTE ON FUNCTION auth.invitation_delivery_preflight(uuid, uuid) TO turnesta_delivery_worker;
GRANT EXECUTE ON FUNCTION auth.finish_invitation_delivery(uuid, uuid, text) TO turnesta_delivery_worker;
