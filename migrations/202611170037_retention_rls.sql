-- The migration login inherits runtime for inspection. Restrictive policies
-- must still allow its trusted maintenance functions to see all old journals.
ALTER POLICY own_command ON bookings.commands
    USING (current_user='turnesta_migrator' OR actor_id=nullif(current_setting('app.user_id',true),'')::uuid)
    WITH CHECK (current_user='turnesta_migrator' OR actor_id=nullif(current_setting('app.user_id',true),'')::uuid);
ALTER POLICY own_command ON bookings.appointment_commands
    USING (current_user='turnesta_migrator' OR actor_id=nullif(current_setting('app.user_id',true),'')::uuid)
    WITH CHECK (current_user='turnesta_migrator' OR actor_id=nullif(current_setting('app.user_id',true),'')::uuid);
ALTER POLICY actor_read ON queue.commands
    USING (current_user='turnesta_migrator' OR actor_id=nullif(current_setting('app.user_id',true),'')::uuid);

CREATE OR REPLACE FUNCTION core.team_replay(p_tenant uuid,p_key uuid,p_command jsonb) RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE actor uuid; previous core.team_commands%ROWTYPE;
BEGIN
    actor:=core.team_require_owner(p_tenant);
    SELECT * INTO previous FROM core.team_commands c WHERE c.tenant_id=p_tenant AND c.actor_id=actor AND c.key=p_key;
    IF FOUND THEN
        IF previous.command<>p_command THEN RAISE EXCEPTION 'key reused' USING ERRCODE='M2202'; END IF;
        RETURN previous.result;
    END IF;
    IF EXISTS(SELECT 1 FROM retention.used_keys t WHERE t.kind='core.team_commands' AND t.tenant_id=p_tenant AND t.scope_id=p_tenant AND t.actor_id=actor::text AND t.key_value=p_key::text) THEN
        RAISE EXCEPTION 'expired key cannot be reused' USING ERRCODE='M2202';
    END IF;
    RETURN NULL;
END $f$;
