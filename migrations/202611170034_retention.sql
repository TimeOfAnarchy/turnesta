-- Retention routines run only through the trusted migration login.
-- Old login codes with a linked delivery are kept until the delivery is gone,
-- avoiding the foreign key's ON DELETE CASCADE on pending or leased work.
CREATE FUNCTION auth.purge_batch(p_kind text, p_dry_run boolean)
RETURNS bigint LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE n bigint; cutoff timestamptz := clock_timestamp() - interval '30 days';
BEGIN
    IF session_user <> 'turnesta_migrator' OR p_dry_run IS NULL THEN
        RAISE EXCEPTION 'maintenance login required' USING ERRCODE='42501';
    END IF;
    CASE p_kind
    WHEN 'sessions' THEN
        IF p_dry_run THEN
            SELECT count(*) INTO n FROM auth.sessions WHERE expires_at < cutoff;
        ELSE
            WITH victims AS (SELECT token_hash FROM auth.sessions WHERE expires_at < cutoff
                ORDER BY expires_at,token_hash LIMIT 1000 FOR UPDATE SKIP LOCKED),
            deleted AS (DELETE FROM auth.sessions s USING victims v WHERE s.token_hash=v.token_hash RETURNING 1)
            SELECT count(*) INTO n FROM deleted;
        END IF;
    WHEN 'password_setup_grants' THEN
        IF p_dry_run THEN
            SELECT count(*) INTO n FROM auth.password_setup_grants WHERE expires_at < cutoff;
        ELSE
            WITH victims AS (SELECT token_hash FROM auth.password_setup_grants WHERE expires_at < cutoff
                ORDER BY expires_at,token_hash LIMIT 1000 FOR UPDATE SKIP LOCKED),
            deleted AS (DELETE FROM auth.password_setup_grants g USING victims v WHERE g.token_hash=v.token_hash RETURNING 1)
            SELECT count(*) INTO n FROM deleted;
        END IF;
    WHEN 'invitation_deliveries' THEN
        IF p_dry_run THEN
            SELECT count(*) INTO n FROM auth.invitation_deliveries
                WHERE state IN ('sent','failed','expired','canceled') AND expires_at < cutoff AND finished_at < cutoff;
        ELSE
            WITH victims AS (SELECT id FROM auth.invitation_deliveries
                WHERE state IN ('sent','failed','expired','canceled') AND expires_at < cutoff AND finished_at < cutoff
                ORDER BY finished_at,id LIMIT 1000 FOR UPDATE SKIP LOCKED),
            deleted AS (DELETE FROM auth.invitation_deliveries d USING victims v WHERE d.id=v.id RETURNING 1)
            SELECT count(*) INTO n FROM deleted;
        END IF;
    WHEN 'login_codes' THEN
        IF p_dry_run THEN
            SELECT count(*) INTO n FROM auth.login_codes c WHERE c.expires_at < cutoff
                AND NOT EXISTS (SELECT 1 FROM auth.invitation_deliveries d WHERE d.code_hash=c.token_hash);
        ELSE
            WITH victims AS (SELECT c.token_hash FROM auth.login_codes c WHERE c.expires_at < cutoff
                AND NOT EXISTS (SELECT 1 FROM auth.invitation_deliveries d WHERE d.code_hash=c.token_hash)
                ORDER BY c.expires_at,c.token_hash LIMIT 1000 FOR UPDATE OF c SKIP LOCKED),
            deleted AS (DELETE FROM auth.login_codes c USING victims v WHERE c.token_hash=v.token_hash RETURNING 1)
            SELECT count(*) INTO n FROM deleted;
        END IF;
    ELSE RAISE EXCEPTION 'unknown retention class' USING ERRCODE='22023';
    END CASE;
    RETURN n;
END $f$;
REVOKE ALL ON FUNCTION auth.purge_batch(text,boolean) FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;
