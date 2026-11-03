-- Global login credentials remain separate from tenant membership.
CREATE TABLE auth.password_credentials (
    identity_id uuid PRIMARY KEY REFERENCES auth.identities(id),
    password_hash text NOT NULL CHECK(password_hash ~ '^\$argon2id\$v=19\$m=19456,t=2,p=1\$[A-Za-z0-9+/]{16,64}\$[A-Za-z0-9+/]{43}$'),
    version bigint NOT NULL DEFAULT 1 CHECK(version>0),
    must_change boolean NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE auth.password_attempt_windows (
    bucket bytea PRIMARY KEY CHECK(octet_length(bucket)=32),
    window_start timestamptz NOT NULL,
    attempts integer NOT NULL CHECK(attempts>0)
);
CREATE INDEX password_attempt_windows_age ON auth.password_attempt_windows(window_start);
CREATE TABLE auth.password_setup_grants (
    token_hash bytea PRIMARY KEY CHECK(octet_length(token_hash)=32),
    identity_id uuid NOT NULL REFERENCES auth.identities(id),
    credential_version bigint NOT NULL,
    expires_at timestamptz NOT NULL,
    used_at timestamptz
);
CREATE INDEX password_setup_grants_identity ON auth.password_setup_grants(identity_id);
REVOKE ALL ON auth.password_credentials,auth.password_attempt_windows,auth.password_setup_grants
    FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;

-- Count before expensive hashing, shared across API processes. No permanent lockout.
CREATE FUNCTION auth.password_budget(p_email text,p_scope text) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE issued timestamptz:=clock_timestamp(); count_now integer; key bytea;
BEGIN
    IF p_scope NOT IN('login','change','setup') OR p_email IS NULL OR octet_length(p_email)>254
    THEN RETURN false; END IF;
    key:=sha256(convert_to('turnesta.password.global:'||p_scope,'UTF8'));
    INSERT INTO auth.password_attempt_windows AS w VALUES(key,issued,1)
    ON CONFLICT(bucket) DO UPDATE SET
        window_start=CASE WHEN w.window_start<=issued-interval '5 minutes' THEN issued ELSE w.window_start END,
        attempts=CASE WHEN w.window_start<=issued-interval '5 minutes' THEN 1 ELSE least(w.attempts+1,1000000) END
    RETURNING attempts INTO count_now;
    IF count_now>200 THEN RETURN false; END IF;
    IF count_now=1 THEN
        DELETE FROM auth.password_attempt_windows WHERE bucket IN(
            SELECT bucket FROM auth.password_attempt_windows WHERE window_start<issued-interval '1 day'
            LIMIT 1000 FOR UPDATE SKIP LOCKED);
    END IF;
    key:=sha256(convert_to('turnesta.password.email:'||p_scope||':'||p_email,'UTF8'));
    INSERT INTO auth.password_attempt_windows AS w VALUES(key,issued,1)
    ON CONFLICT(bucket) DO UPDATE SET
        window_start=CASE WHEN w.window_start<=issued-interval '5 minutes' THEN issued ELSE w.window_start END,
        attempts=CASE WHEN w.window_start<=issued-interval '5 minutes' THEN 1 ELSE least(w.attempts+1,1000000) END
    RETURNING attempts INTO count_now;
    RETURN count_now<=10;
END $f$;

CREATE FUNCTION auth.password_lookup(p_email text)
RETURNS TABLE(identity_id uuid,user_id uuid,password_hash text,version bigint,must_change boolean)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $f$
    SELECT i.id,i.user_id,c.password_hash,c.version,c.must_change
    FROM auth.identities i JOIN auth.password_credentials c ON c.identity_id=i.id
    WHERE i.provider='email' AND i.subject=p_email
$f$;

CREATE FUNCTION auth.password_for_session(p_session bytea)
RETURNS TABLE(email text,password_hash text,version bigint)
LANGUAGE sql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
    SELECT i.subject,c.password_hash,coalesce(c.version,0) FROM auth.find_session(p_session) s
    JOIN auth.identities i ON i.user_id=s.user_id AND i.provider='email'
    LEFT JOIN auth.password_credentials c ON c.identity_id=i.id
$f$;

-- Lock identity before credential everywhere, including the existing magic-link path.
CREATE FUNCTION auth.password_finish_login(p_identity uuid,p_version bigint,p_session bytea,p_csrf bytea,p_ttl integer,p_previous bytea)
RETURNS uuid LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE target uuid;
BEGIN
    SELECT i.user_id INTO target FROM auth.identities i WHERE i.id=p_identity FOR UPDATE;
    PERFORM 1 FROM auth.password_credentials c WHERE c.identity_id=p_identity AND c.version=p_version AND NOT c.must_change FOR UPDATE;
    IF NOT FOUND THEN RETURN NULL; END IF;
    PERFORM auth.create_session(target,p_session,p_csrf,p_ttl);
    IF p_previous IS NOT NULL THEN PERFORM auth.revoke_session(p_previous); END IF;
    RETURN target;
END $f$;

CREATE FUNCTION auth.password_setup_begin(p_identity uuid,p_version bigint,p_grant bytea)
RETURNS boolean LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
BEGIN
    PERFORM 1 FROM auth.identities WHERE id=p_identity FOR UPDATE;
    PERFORM 1 FROM auth.password_credentials c WHERE c.identity_id=p_identity AND c.version=p_version AND c.must_change FOR UPDATE;
    IF NOT FOUND THEN RETURN false; END IF;
    UPDATE auth.password_setup_grants SET used_at=clock_timestamp() WHERE identity_id=p_identity AND used_at IS NULL;
    INSERT INTO auth.password_setup_grants VALUES(p_grant,p_identity,p_version,clock_timestamp()+interval '10 minutes',NULL);
    RETURN true;
END $f$;

CREATE FUNCTION auth.password_setup_lookup(p_grant bytea)
RETURNS TABLE(email text,password_hash text,version bigint)
LANGUAGE sql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
    SELECT i.subject,c.password_hash,c.version FROM auth.password_setup_grants g JOIN auth.password_credentials c ON c.identity_id=g.identity_id
        JOIN auth.identities i ON i.id=g.identity_id
        WHERE g.token_hash=p_grant AND g.used_at IS NULL AND g.expires_at>clock_timestamp()
        AND g.credential_version=c.version AND c.must_change
$f$;

CREATE FUNCTION auth.password_revoke_access(p_identity uuid) RETURNS void
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
BEGIN
    UPDATE auth.sessions SET revoked_at=clock_timestamp() WHERE user_id=(SELECT user_id FROM auth.identities WHERE id=p_identity) AND revoked_at IS NULL;
    UPDATE auth.login_codes SET used_at=clock_timestamp() WHERE identity_id=p_identity AND used_at IS NULL;
    UPDATE auth.password_setup_grants SET used_at=clock_timestamp() WHERE identity_id=p_identity AND used_at IS NULL;
END $f$;

CREATE FUNCTION auth.password_setup_finish(p_grant bytea,p_version bigint,p_hash text,p_session bytea,p_csrf bytea,p_ttl integer,p_previous bytea)
RETURNS uuid LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE grant_row auth.password_setup_grants%ROWTYPE; target uuid; identity uuid;
BEGIN
    SELECT identity_id INTO identity FROM auth.password_setup_grants WHERE token_hash=p_grant;
    SELECT i.user_id INTO target FROM auth.identities i WHERE i.id=identity FOR UPDATE;
    PERFORM 1 FROM auth.password_credentials c WHERE c.identity_id=identity AND c.version=p_version AND c.must_change FOR UPDATE;
    IF NOT FOUND THEN RETURN NULL; END IF;
    SELECT * INTO grant_row FROM auth.password_setup_grants WHERE token_hash=p_grant AND used_at IS NULL AND expires_at>clock_timestamp() FOR UPDATE;
    IF NOT FOUND OR grant_row.credential_version<>p_version THEN RETURN NULL; END IF;
    UPDATE auth.password_credentials SET password_hash=p_hash,version=version+1,must_change=false,updated_at=clock_timestamp() WHERE identity_id=identity;
    PERFORM auth.password_revoke_access(identity);
    PERFORM auth.create_session(target,p_session,p_csrf,p_ttl);
    IF p_previous IS NOT NULL THEN PERFORM auth.revoke_session(p_previous); END IF;
    RETURN target;
END $f$;

CREATE FUNCTION auth.password_change(p_session bytea,p_version bigint,p_hash text) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE identity uuid; actual bigint;
BEGIN
    SELECT i.id INTO identity FROM auth.find_session(p_session) s JOIN auth.identities i ON i.user_id=s.user_id AND i.provider='email';
    IF identity IS NULL THEN RETURN false; END IF;
    PERFORM 1 FROM auth.identities WHERE id=identity FOR UPDATE;
    PERFORM 1 FROM auth.find_session(p_session);
    IF NOT FOUND THEN RETURN false; END IF;
    SELECT version INTO actual FROM auth.password_credentials WHERE identity_id=identity FOR UPDATE;
    IF coalesce(actual,0)<>p_version THEN RETURN false; END IF;
    INSERT INTO auth.password_credentials(identity_id,password_hash,must_change) VALUES(identity,p_hash,false)
    ON CONFLICT(identity_id) DO UPDATE SET password_hash=p_hash,version=auth.password_credentials.version+1,must_change=false,updated_at=clock_timestamp();
    PERFORM auth.password_revoke_access(identity);
    RETURN true;
END $f$;

-- Trusted platform operator only; a tenant owner cannot reset global accounts.
CREATE FUNCTION auth.password_admin_set(p_user uuid,p_email text,p_hash text) RETURNS void
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE identity uuid;
BEGIN
    PERFORM auth.bind_email(p_user,p_email);
    SELECT id INTO identity FROM auth.identities WHERE user_id=p_user AND provider='email' FOR UPDATE;
    INSERT INTO auth.password_credentials(identity_id,password_hash,must_change) VALUES(identity,p_hash,true)
    ON CONFLICT(identity_id) DO UPDATE SET password_hash=p_hash,version=auth.password_credentials.version+1,must_change=true,updated_at=clock_timestamp();
    PERFORM auth.password_revoke_access(identity);
END $f$;

CREATE FUNCTION core.team_password_account(p_tenant uuid,p_email text,p_name text,p_role text,p_hash text) RETURNS uuid
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE actor uuid; target uuid:=gen_random_uuid(); identity uuid;
BEGIN
    actor:=core.team_require_owner(p_tenant);
    PERFORM 1 FROM core.organizations WHERE id=p_tenant FOR UPDATE;
    IF p_email IS NULL OR p_email<>lower(btrim(p_email)) OR octet_length(p_email)<>char_length(p_email) OR char_length(p_email) NOT BETWEEN 3 AND 254
        OR p_email !~ '^[^@[:space:][:cntrl:]]+@[^@[:space:][:cntrl:]]+$'
        OR p_name IS NULL OR p_name<>btrim(p_name) OR char_length(p_name) NOT BETWEEN 1 AND 120 OR p_name~'[[:cntrl:]]'
        OR p_role IS NULL OR p_role NOT IN('manager','operator','employee')
    THEN RAISE EXCEPTION 'invalid account details' USING ERRCODE='P2301'; END IF;
    IF (SELECT count(*) FROM core.team_commands WHERE tenant_id=p_tenant AND command->>'type' IN('invite','password_account')
        AND created_at>clock_timestamp()-interval '1 hour')>=20
    THEN RAISE EXCEPTION 'account creation rate exceeded' USING ERRCODE='P2303'; END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended('turnesta.team.email:'||p_email,0));
    IF EXISTS(SELECT 1 FROM auth.identities WHERE provider='email' AND subject=p_email)
    THEN RAISE EXCEPTION 'email already has an account' USING ERRCODE='P2302'; END IF;
    INSERT INTO core.users(id,display_name) VALUES(target,p_name);
    INSERT INTO auth.identities(provider,subject,user_id) VALUES('email',p_email,target) RETURNING id INTO identity;
    INSERT INTO core.memberships(tenant_id,user_id,role) VALUES(p_tenant,target,p_role);
    INSERT INTO auth.password_credentials(identity_id,password_hash,must_change) VALUES(identity,p_hash,true);
    INSERT INTO core.team_commands(tenant_id,actor_id,key,command,result)
        VALUES(p_tenant,actor,gen_random_uuid(),jsonb_build_object('type','password_account','email',p_email,'role',p_role),jsonb_build_object('user_id',target));
    RETURN target;
END $f$;

REVOKE ALL ON FUNCTION auth.password_budget(text,text),auth.password_lookup(text),auth.password_for_session(bytea),
    auth.password_finish_login(uuid,bigint,bytea,bytea,integer,bytea),auth.password_setup_begin(uuid,bigint,bytea),
    auth.password_setup_lookup(bytea),auth.password_revoke_access(uuid),auth.password_setup_finish(bytea,bigint,text,bytea,bytea,integer,bytea),
    auth.password_change(bytea,bigint,text),auth.password_admin_set(uuid,text,text),core.team_password_account(uuid,text,text,text,text)
    FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;
GRANT EXECUTE ON FUNCTION auth.password_budget(text,text),auth.password_lookup(text),auth.password_for_session(bytea),
    auth.password_finish_login(uuid,bigint,bytea,bytea,integer,bytea),auth.password_setup_begin(uuid,bigint,bytea),
    auth.password_setup_lookup(bytea),auth.password_setup_finish(bytea,bigint,text,bytea,bytea,integer,bytea),auth.password_change(bytea,bigint,text),
    core.team_password_account(uuid,text,text,text,text) TO turnesta_runtime;
GRANT EXECUTE ON FUNCTION auth.password_admin_set(uuid,text,text) TO turnesta_provisioner;

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

    -- A temporary password grants only password setup, through its own flow.
    IF EXISTS(SELECT 1 FROM auth.password_credentials WHERE identity_id=bound_identity AND must_change) THEN
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
