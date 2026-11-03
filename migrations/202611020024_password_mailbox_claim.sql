-- Keep account passwords global, but require mailbox proof before tenant reuse.
-- Preserve credential revisions when an unverified, owner-issued password is disabled.
ALTER TABLE auth.password_credentials ADD COLUMN enabled boolean NOT NULL DEFAULT true;
ALTER TABLE auth.sessions ADD COLUMN authentication_method text NOT NULL DEFAULT 'legacy'
    CHECK(authentication_method IN('legacy','email','password'));

CREATE FUNCTION auth.password_owner_session(p_session bytea)
RETURNS TABLE(email text,password_hash text,version bigint,requires_current_password boolean,password_enabled boolean)
LANGUAGE sql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
    SELECT i.subject,c.password_hash,coalesce(c.version,0),
        coalesce(c.enabled,false) AND session.authentication_method<>'email',coalesce(c.enabled,false)
    FROM auth.find_session(p_session) valid JOIN auth.sessions session ON session.token_hash=p_session
    JOIN auth.identities i ON i.user_id=valid.user_id AND i.provider='email'
    LEFT JOIN auth.password_credentials c ON c.identity_id=i.id
    WHERE i.verified_at IS NOT NULL OR c.enabled
$f$;
REVOKE ALL ON FUNCTION auth.password_owner_session(bytea) FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;
GRANT EXECUTE ON FUNCTION auth.password_owner_session(bytea) TO turnesta_runtime;
REVOKE ALL ON FUNCTION auth.password_for_session(bytea) FROM turnesta_runtime;


CREATE OR REPLACE FUNCTION auth.password_lookup(p_email text)
RETURNS TABLE(identity_id uuid,user_id uuid,password_hash text,version bigint,must_change boolean)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $f$
    SELECT i.id,i.user_id,c.password_hash,c.version,c.must_change
    FROM auth.identities i JOIN auth.password_credentials c ON c.identity_id=i.id
    WHERE i.provider='email' AND i.subject=p_email AND c.enabled
$f$;

CREATE OR REPLACE FUNCTION auth.password_setup_lookup(p_grant bytea)
RETURNS TABLE(email text,password_hash text,version bigint)
LANGUAGE sql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
    SELECT i.subject,c.password_hash,c.version FROM auth.password_setup_grants g JOIN auth.password_credentials c ON c.identity_id=g.identity_id
        JOIN auth.identities i ON i.id=g.identity_id
        WHERE g.token_hash=p_grant AND g.used_at IS NULL AND g.expires_at>clock_timestamp()
        AND g.credential_version=c.version AND c.must_change AND c.enabled
$f$;

CREATE OR REPLACE FUNCTION auth.password_finish_login(p_identity uuid,p_version bigint,p_session bytea,p_csrf bytea,p_ttl integer,p_previous bytea)
RETURNS uuid LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE target uuid;
BEGIN
    SELECT i.user_id INTO target FROM auth.identities i WHERE i.id=p_identity FOR UPDATE;
    PERFORM 1 FROM auth.password_credentials c WHERE c.identity_id=p_identity AND c.version=p_version AND NOT c.must_change AND c.enabled FOR UPDATE;
    IF NOT FOUND THEN RETURN NULL; END IF;
    PERFORM auth.create_session(target,p_session,p_csrf,p_ttl);
    UPDATE auth.sessions SET authentication_method='password' WHERE token_hash=p_session;
    IF p_previous IS NOT NULL THEN PERFORM auth.revoke_session(p_previous); END IF;
    RETURN target;
END $f$;

CREATE OR REPLACE FUNCTION auth.password_setup_begin(p_identity uuid,p_version bigint,p_grant bytea)
RETURNS boolean LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
BEGIN
    PERFORM 1 FROM auth.identities WHERE id=p_identity FOR UPDATE;
    PERFORM 1 FROM auth.password_credentials c WHERE c.identity_id=p_identity AND c.version=p_version AND c.must_change AND c.enabled FOR UPDATE;
    IF NOT FOUND THEN RETURN false; END IF;
    UPDATE auth.password_setup_grants SET used_at=clock_timestamp() WHERE identity_id=p_identity AND used_at IS NULL;
    INSERT INTO auth.password_setup_grants VALUES(p_grant,p_identity,p_version,clock_timestamp()+interval '10 minutes',NULL);
    RETURN true;
END $f$;

CREATE OR REPLACE FUNCTION auth.password_setup_finish(p_grant bytea,p_version bigint,p_hash text,p_session bytea,p_csrf bytea,p_ttl integer,p_previous bytea)
RETURNS uuid LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE grant_row auth.password_setup_grants%ROWTYPE; target uuid; identity uuid;
BEGIN
    SELECT identity_id INTO identity FROM auth.password_setup_grants WHERE token_hash=p_grant;
    SELECT i.user_id INTO target FROM auth.identities i WHERE i.id=identity FOR UPDATE;
    PERFORM 1 FROM auth.password_credentials c WHERE c.identity_id=identity AND c.version=p_version AND c.must_change AND c.enabled FOR UPDATE;
    IF NOT FOUND THEN RETURN NULL; END IF;
    SELECT * INTO grant_row FROM auth.password_setup_grants WHERE token_hash=p_grant AND used_at IS NULL AND expires_at>clock_timestamp() FOR UPDATE;
    IF NOT FOUND OR grant_row.credential_version<>p_version THEN RETURN NULL; END IF;
    UPDATE auth.password_credentials SET password_hash=p_hash,version=version+1,must_change=false,enabled=true,updated_at=clock_timestamp() WHERE identity_id=identity;
    PERFORM auth.password_revoke_access(identity);
    PERFORM auth.create_session(target,p_session,p_csrf,p_ttl);
    UPDATE auth.sessions SET authentication_method='password' WHERE token_hash=p_session;
    IF p_previous IS NOT NULL THEN PERFORM auth.revoke_session(p_previous); END IF;
    RETURN target;
END $f$;

CREATE OR REPLACE FUNCTION auth.password_change(p_session bytea,p_version bigint,p_hash text) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE identity uuid; actual bigint;
BEGIN
    SELECT i.id INTO identity FROM auth.find_session(p_session) s JOIN auth.identities i ON i.user_id=s.user_id AND i.provider='email';
    IF identity IS NULL THEN RETURN false; END IF;
    PERFORM 1 FROM auth.identities WHERE id=identity FOR UPDATE;
    PERFORM 1 FROM auth.find_session(p_session);
    IF NOT FOUND THEN RETURN false; END IF;
    IF NOT EXISTS(SELECT 1 FROM auth.identities i LEFT JOIN auth.password_credentials c ON c.identity_id=i.id
        WHERE i.id=identity AND (i.verified_at IS NOT NULL OR c.enabled)) THEN RETURN false; END IF;
    SELECT version INTO actual FROM auth.password_credentials WHERE identity_id=identity FOR UPDATE;
    IF coalesce(actual,0)<>p_version THEN RETURN false; END IF;
    INSERT INTO auth.password_credentials(identity_id,password_hash,must_change) VALUES(identity,p_hash,false)
    ON CONFLICT(identity_id) DO UPDATE SET password_hash=p_hash,version=auth.password_credentials.version+1,must_change=false,enabled=true,updated_at=clock_timestamp();
    PERFORM auth.password_revoke_access(identity);
    RETURN true;
END $f$;

CREATE OR REPLACE FUNCTION auth.password_admin_set(p_user uuid,p_email text,p_hash text) RETURNS void
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE identity uuid;
BEGIN
    PERFORM auth.bind_email(p_user,p_email);
    SELECT id INTO identity FROM auth.identities WHERE user_id=p_user AND provider='email' FOR UPDATE;
    INSERT INTO auth.password_credentials(identity_id,password_hash,must_change) VALUES(identity,p_hash,true)
    ON CONFLICT(identity_id) DO UPDATE SET password_hash=p_hash,version=auth.password_credentials.version+1,must_change=true,enabled=true,updated_at=clock_timestamp();
    PERFORM auth.password_revoke_access(identity);
END $f$;

CREATE OR REPLACE FUNCTION core.team_execute(p_tenant uuid,p_key uuid,p_command jsonb,p_job uuid,p_hash bytea,p_key_id bytea,p_nonce bytea,p_ciphertext bytea)
RETURNS jsonb LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE actor uuid; previous jsonb; target core.memberships%ROWTYPE; target_user uuid; bound uuid; email text;
    action text:=p_command->>'type'; output jsonb; delivery jsonb; label text;
BEGIN
    -- One organization serializes membership management and the command journal.
    PERFORM 1 FROM core.organizations o WHERE o.id=p_tenant FOR UPDATE;
    actor:=core.team_require_owner(p_tenant);
    PERFORM 1 FROM core.memberships m WHERE m.tenant_id=p_tenant AND m.user_id=actor FOR SHARE;
    previous:=core.team_replay(p_tenant,p_key,p_command);
    IF previous IS NOT NULL THEN RETURN previous; END IF;
    IF p_key IS NULL OR p_command IS NULL OR action IS NULL OR action NOT IN('invite','set_member','send_login')
    THEN RAISE EXCEPTION 'invalid team command' USING ERRCODE='M2205'; END IF;
    IF action='invite' THEN
        email:=p_command->>'email'; label:=p_command->>'display_name';
        IF email IS NULL OR length(email)>254 OR email<>lower(btrim(email)) OR email !~ '^[^@[:space:][:cntrl:]]+@[^@[:space:][:cntrl:]]+$'
            OR label IS NULL OR char_length(label) NOT BETWEEN 1 AND 120 OR label<>btrim(label) OR label~'[[:cntrl:]]'
            OR p_command->>'role' IS NULL OR p_command->>'role' NOT IN('manager','operator','employee')
        THEN RAISE EXCEPTION 'invalid invitation' USING ERRCODE='M2205'; END IF;
        IF (SELECT count(*) FROM core.team_commands c WHERE c.tenant_id=p_tenant AND c.command->>'type'='invite'
            AND c.created_at>clock_timestamp()-interval '1 hour')>=20
        THEN RAISE EXCEPTION 'team invitation rate exceeded' USING ERRCODE='M2204'; END IF;
        PERFORM pg_advisory_xact_lock(hashtextextended('turnesta.team.email:'||email,0));
        SELECT i.user_id INTO target_user FROM auth.identities i WHERE i.provider='email' AND i.subject=email FOR UPDATE;
        IF target_user IS NOT NULL AND EXISTS(SELECT 1 FROM auth.identities i JOIN auth.password_credentials c ON c.identity_id=i.id
            WHERE i.user_id=target_user AND i.verified_at IS NULL)
        THEN RAISE EXCEPTION 'mailbox ownership must be confirmed before another organization can grant access' USING ERRCODE='P2401'; END IF;
        IF target_user IS NULL THEN
            target_user:=gen_random_uuid();
            INSERT INTO core.users(id,display_name) VALUES(target_user,label);
            PERFORM auth.bind_email(target_user,email);
        END IF;
        IF EXISTS(SELECT 1 FROM core.memberships m WHERE m.tenant_id=p_tenant AND m.user_id=target_user)
        THEN RAISE EXCEPTION 'member already exists' USING ERRCODE='M2206'; END IF;
        INSERT INTO core.memberships(tenant_id,user_id,role) VALUES(p_tenant,target_user,p_command->>'role') RETURNING * INTO target;
        delivery:=core.team_queue_email(p_tenant,target_user,email,p_job,p_hash,p_key_id,p_nonce,p_ciphertext);
    ELSE
        target_user:=(p_command->>'user_id')::uuid;
        SELECT * INTO target FROM core.memberships m WHERE m.tenant_id=p_tenant AND m.user_id=target_user FOR UPDATE;
        IF NOT FOUND THEN RAISE EXCEPTION 'member not found' USING ERRCODE='M2203'; END IF;
        IF target.role='owner' OR target_user=actor THEN RAISE EXCEPTION 'owner is protected' USING ERRCODE='M2207'; END IF;
        IF target.version IS DISTINCT FROM (p_command->>'expected_version')::integer
        THEN RAISE EXCEPTION 'member changed' USING ERRCODE='M2208'; END IF;
        IF action='set_member' THEN
            IF p_command->>'role' IS NULL OR p_command->>'role' NOT IN('manager','operator','employee') OR jsonb_typeof(p_command->'active') IS DISTINCT FROM 'boolean'
            THEN RAISE EXCEPTION 'invalid role or state' USING ERRCODE='M2205'; END IF;
            UPDATE core.memberships m SET role=p_command->>'role',active=(p_command->>'active')::boolean,version=m.version+1
            WHERE m.tenant_id=p_tenant AND m.user_id=target_user RETURNING * INTO target;
        ELSE
            email:=core.team_email(p_tenant,target_user);
            PERFORM 1 FROM auth.identities i WHERE i.provider='email' AND i.subject=email FOR UPDATE;
            delivery:=core.team_queue_email(p_tenant,target_user,email,p_job,p_hash,p_key_id,p_nonce,p_ciphertext);
            UPDATE core.memberships m SET version=m.version+1 WHERE m.tenant_id=p_tenant AND m.user_id=target_user RETURNING * INTO target;
        END IF;
    END IF;
    output:=jsonb_build_object('user_id',target.user_id,'role',target.role,'active',target.active,'version',target.version,'delivery',delivery);
    INSERT INTO core.team_commands(tenant_id,actor_id,key,command,result) VALUES(p_tenant,actor,p_key,p_command,output);
    RETURN output;
END $f$;

CREATE OR REPLACE FUNCTION auth.consume_login_code(
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

    -- A first mailbox proof supersedes credentials known to the account issuer.
    -- A fresh email proof also recovers an account after an operator password reset.
    IF EXISTS(SELECT 1 FROM auth.password_credentials c JOIN auth.identities i ON i.id=c.identity_id
        WHERE i.id=bound_identity AND (i.verified_at IS NULL OR c.must_change)) THEN
        PERFORM 1 FROM auth.password_credentials WHERE identity_id=bound_identity FOR UPDATE;
        UPDATE auth.password_credentials SET enabled=false,version=version+1,must_change=true,updated_at=clock_timestamp()
            WHERE identity_id=bound_identity;
        PERFORM auth.password_revoke_access(bound_identity);
    END IF;
    UPDATE auth.identities AS identity
    SET verified_at = COALESCE(identity.verified_at, pg_catalog.clock_timestamp())
    WHERE identity.id = bound_identity;
    PERFORM auth.create_session(bound_user, p_session_hash, p_csrf_hash, p_session_ttl_seconds);
    UPDATE auth.sessions SET authentication_method='email' WHERE token_hash=p_session_hash;
    IF p_previous_session_hash IS NOT NULL THEN
        PERFORM auth.revoke_session(p_previous_session_hash);
    END IF;
    RETURN bound_user;
END
$function$;
