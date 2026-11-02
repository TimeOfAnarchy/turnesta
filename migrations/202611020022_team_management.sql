-- Preserve historical membership references when access is withdrawn.
ALTER TABLE core.memberships ADD COLUMN active boolean NOT NULL DEFAULT true;
ALTER TABLE core.memberships ADD COLUMN version integer NOT NULL DEFAULT 1 CHECK(version>0);
ALTER POLICY own_membership ON core.memberships USING (
    active AND tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
    AND user_id=nullif(current_setting('app.user_id',true),'')::uuid);

CREATE OR REPLACE FUNCTION core.list_user_organizations(p_user_id uuid,p_after_id uuid,p_limit integer)
RETURNS TABLE(id uuid,name text,role text) LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $f$
    SELECT o.id,o.name,m.role FROM core.organizations o JOIN core.memberships m ON m.tenant_id=o.id
    WHERE m.user_id=p_user_id AND m.active AND (p_after_id IS NULL OR o.id>p_after_id)
    ORDER BY o.id LIMIT least(greatest(p_limit,1),100)+1
$f$;

-- This existing definer checks membership without the runtime's RLS policy.
CREATE OR REPLACE FUNCTION tips.reconcile_attempt(p_tenant_id uuid,p_attempt_id uuid)
RETURNS void LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE payload jsonb;
BEGIN
    IF p_tenant_id IS DISTINCT FROM nullif(current_setting('app.tenant_id',true),'')::uuid
        OR NOT EXISTS(SELECT 1 FROM core.memberships m WHERE m.tenant_id=p_tenant_id
            AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.active AND m.role IN('owner','manager'))
    THEN RAISE EXCEPTION 'tips management requires owner or manager' USING ERRCODE='T1003'; END IF;
    IF NOT EXISTS(SELECT 1 FROM tips.attempts a WHERE a.tenant_id=p_tenant_id AND a.id=p_attempt_id)
    THEN RAISE EXCEPTION 'mock attempt not found' USING ERRCODE='T1002'; END IF;
    PERFORM tips.expire_mock_attempt(p_attempt_id);
    SELECT l.event_payload INTO payload FROM tips.mock_ledger l WHERE l.attempt_id=p_attempt_id;
    IF payload IS NOT NULL THEN PERFORM tips.apply_mock_event(payload); END IF;
END $f$;

CREATE TABLE core.team_commands (
    tenant_id uuid NOT NULL,
    actor_id uuid NOT NULL,
    key uuid NOT NULL,
    command jsonb NOT NULL,
    result jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(tenant_id,actor_id,key),
    FOREIGN KEY(tenant_id,actor_id) REFERENCES core.memberships(tenant_id,user_id)
);
CREATE INDEX team_commands_rate_idx ON core.team_commands(tenant_id,created_at);
CREATE TABLE core.team_invitations (
    tenant_id uuid NOT NULL,
    user_id uuid NOT NULL,
    job_id uuid REFERENCES auth.invitation_deliveries(id),
    deferred_until timestamptz,
    sent_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(tenant_id,user_id),
    FOREIGN KEY(tenant_id,user_id) REFERENCES core.memberships(tenant_id,user_id)
);
REVOKE ALL ON core.team_commands,core.team_invitations FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;
ALTER TABLE core.team_commands ENABLE ROW LEVEL SECURITY;
ALTER TABLE core.team_commands FORCE ROW LEVEL SECURITY;
ALTER TABLE core.team_invitations ENABLE ROW LEVEL SECURITY;
ALTER TABLE core.team_invitations FORCE ROW LEVEL SECURITY;
CREATE POLICY team_owner ON core.team_commands TO CURRENT_USER USING(true) WITH CHECK(true);
CREATE POLICY team_owner ON core.team_invitations TO CURRENT_USER USING(true) WITH CHECK(true);

CREATE FUNCTION core.team_require_owner(p_tenant uuid) RETURNS uuid
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE actor uuid:=nullif(current_setting('app.user_id',true),'')::uuid;
BEGIN
    IF p_tenant IS DISTINCT FROM nullif(current_setting('app.tenant_id',true),'')::uuid
        OR NOT EXISTS(SELECT 1 FROM core.memberships m WHERE m.tenant_id=p_tenant AND m.user_id=actor AND m.active AND m.role='owner')
    THEN RAISE EXCEPTION 'team management requires owner' USING ERRCODE='M2201'; END IF;
    RETURN actor;
END $f$;

CREATE FUNCTION core.team_replay(p_tenant uuid,p_key uuid,p_command jsonb) RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE actor uuid; previous core.team_commands%ROWTYPE;
BEGIN
    actor:=core.team_require_owner(p_tenant);
    SELECT * INTO previous FROM core.team_commands c WHERE c.tenant_id=p_tenant AND c.actor_id=actor AND c.key=p_key;
    IF FOUND THEN
        IF previous.command<>p_command THEN RAISE EXCEPTION 'key reused' USING ERRCODE='M2202'; END IF;
        RETURN previous.result;
    END IF;
    RETURN NULL;
END $f$;

CREATE FUNCTION core.team_email(p_tenant uuid,p_user uuid) RETURNS text
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE email text;
BEGIN
    PERFORM core.team_require_owner(p_tenant);
    SELECT i.subject INTO email FROM core.memberships m JOIN auth.identities i ON i.user_id=m.user_id AND i.provider='email'
    WHERE m.tenant_id=p_tenant AND m.user_id=p_user AND m.role<>'owner' AND m.active;
    IF email IS NULL THEN RAISE EXCEPTION 'member cannot receive login email' USING ERRCODE='M2203'; END IF;
    RETURN email;
END $f$;

CREATE FUNCTION core.team_members(p_tenant uuid,p_after uuid,p_limit integer)
RETURNS TABLE(user_id uuid,display_name text,email text,role text,active boolean,version integer,
    delivery_state text,delivery_attempts integer,delivery_expires_at text,resend_after text)
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $f$
BEGIN
    PERFORM core.team_require_owner(p_tenant);
    RETURN QUERY SELECT m.user_id,u.display_name,i.subject,m.role,m.active,m.version,
        CASE WHEN t.deferred_until IS NOT NULL THEN 'deferred' ELSE j.state END,j.attempts,
        to_jsonb(j.expires_at)#>>'{}',to_jsonb(greatest(t.sent_at+interval '60 seconds',t.deferred_until))#>>'{}'
    FROM core.memberships m JOIN core.users u ON u.id=m.user_id
    LEFT JOIN auth.identities i ON i.user_id=m.user_id AND i.provider='email'
    LEFT JOIN core.team_invitations t ON t.tenant_id=m.tenant_id AND t.user_id=m.user_id
    LEFT JOIN auth.invitation_deliveries j ON j.id=t.job_id
    WHERE m.tenant_id=p_tenant AND (p_after IS NULL OR m.user_id>p_after)
    ORDER BY m.user_id LIMIT least(greatest(p_limit,1),100)+1;
END $f$;

-- Called only by team_execute, after identity locking. Do not cancel an active
-- global login request belonging to another organization or the trusted CLI.
CREATE FUNCTION core.team_queue_email(p_tenant uuid,p_user uuid,p_email text,p_job uuid,p_hash bytea,p_key_id bytea,p_nonce bytea,p_ciphertext bytea)
RETURNS jsonb LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE previous core.team_invitations%ROWTYPE; wait_until timestamptz;
BEGIN
    SELECT * INTO previous FROM core.team_invitations t WHERE t.tenant_id=p_tenant AND t.user_id=p_user;
    IF greatest(previous.sent_at+interval '60 seconds',previous.deferred_until)>clock_timestamp()
    THEN RAISE EXCEPTION 'wait before sending another email' USING ERRCODE='M2204'; END IF;
    IF (SELECT count(*) FROM core.team_commands c WHERE c.tenant_id=p_tenant AND c.created_at>clock_timestamp()-interval '1 hour'
        AND c.command->>'type' IN('invite','send_login'))>=20
    THEN RAISE EXCEPTION 'team invitation rate exceeded' USING ERRCODE='M2204'; END IF;
    SELECT max(c.expires_at) INTO wait_until FROM auth.login_codes c JOIN auth.identities i ON i.id=c.identity_id
    WHERE i.provider='email' AND i.subject=p_email AND c.used_at IS NULL AND c.expires_at>clock_timestamp()
        AND NOT EXISTS(SELECT 1 FROM auth.invitation_deliveries j WHERE j.id=previous.job_id AND j.code_hash=c.token_hash);
    IF wait_until IS NOT NULL THEN
        INSERT INTO core.team_invitations(tenant_id,user_id,deferred_until) VALUES(p_tenant,p_user,wait_until)
        ON CONFLICT(tenant_id,user_id) DO UPDATE SET job_id=NULL,deferred_until=EXCLUDED.deferred_until,sent_at=clock_timestamp();
        RETURN jsonb_build_object('state','deferred','retry_after',wait_until);
    END IF;
    PERFORM auth.enqueue_invitation(p_job,p_user,p_email,p_hash,p_key_id,p_nonce,p_ciphertext);
    INSERT INTO core.team_invitations(tenant_id,user_id,job_id) VALUES(p_tenant,p_user,p_job)
    ON CONFLICT(tenant_id,user_id) DO UPDATE SET job_id=EXCLUDED.job_id,deferred_until=NULL,sent_at=clock_timestamp();
    RETURN jsonb_build_object('state','queued','retry_after',clock_timestamp()+interval '60 seconds');
END $f$;

CREATE FUNCTION core.team_execute(p_tenant uuid,p_key uuid,p_command jsonb,p_job uuid,p_hash bytea,p_key_id bytea,p_nonce bytea,p_ciphertext bytea)
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

REVOKE ALL ON FUNCTION core.team_require_owner(uuid),core.team_queue_email(uuid,uuid,text,uuid,bytea,bytea,bytea,bytea),
    core.team_replay(uuid,uuid,jsonb),core.team_email(uuid,uuid),core.team_members(uuid,uuid,integer),
    core.team_execute(uuid,uuid,jsonb,uuid,bytea,bytea,bytea,bytea) FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;
GRANT EXECUTE ON FUNCTION core.team_replay(uuid,uuid,jsonb),core.team_email(uuid,uuid),core.team_members(uuid,uuid,integer),
    core.team_execute(uuid,uuid,jsonb,uuid,bytea,bytea,bytea,bytea) TO turnesta_runtime;
