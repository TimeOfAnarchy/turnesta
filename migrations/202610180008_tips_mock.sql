CREATE SCHEMA tips;
REVOKE ALL ON SCHEMA tips FROM PUBLIC;
GRANT USAGE ON SCHEMA tips TO turnesta_runtime, turnesta_delivery_worker;

CREATE TABLE tips.recipients (
    tenant_id uuid NOT NULL,
    id uuid NOT NULL,
    location_id uuid NOT NULL,
    display_name text NOT NULL CHECK (char_length(display_name) BETWEEN 1 AND 120 AND display_name = btrim(display_name) AND display_name !~ '[[:cntrl:]]'),
    public_slug uuid NOT NULL UNIQUE,
    environment text NOT NULL DEFAULT 'mock' CHECK (environment = 'mock'),
    PRIMARY KEY (tenant_id, id), UNIQUE (id), UNIQUE (tenant_id, id, location_id),
    FOREIGN KEY (tenant_id, location_id) REFERENCES core.locations (tenant_id, id)
);
CREATE TABLE tips.attempts (
    tenant_id uuid NOT NULL,
    id uuid NOT NULL,
    recipient_id uuid NOT NULL,
    location_id uuid NOT NULL,
    idempotency_key uuid NOT NULL,
    amount_minor bigint NOT NULL CHECK (amount_minor BETWEEN 100 AND 1000000),
    currency text NOT NULL DEFAULT 'RUB' CHECK (currency = 'RUB'),
    status text NOT NULL DEFAULT 'awaiting_payment' CHECK (status IN ('awaiting_payment', 'succeeded', 'failed', 'expired')),
    environment text NOT NULL DEFAULT 'mock' CHECK (environment = 'mock'),
    checkout_token uuid NOT NULL UNIQUE,
    status_token uuid NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT pg_catalog.clock_timestamp(),
    expires_at timestamptz NOT NULL DEFAULT (pg_catalog.clock_timestamp() + interval '30 minutes'),
    PRIMARY KEY (tenant_id, id), UNIQUE (id), UNIQUE (tenant_id, recipient_id, idempotency_key),
    FOREIGN KEY (tenant_id, recipient_id, location_id) REFERENCES tips.recipients (tenant_id, id, location_id),
    FOREIGN KEY (tenant_id, location_id) REFERENCES core.locations (tenant_id, id)
);
ALTER TABLE tips.recipients ENABLE ROW LEVEL SECURITY;
ALTER TABLE tips.recipients FORCE ROW LEVEL SECURITY;
ALTER TABLE tips.attempts ENABLE ROW LEVEL SECURITY;
ALTER TABLE tips.attempts FORCE ROW LEVEL SECURITY;
CREATE POLICY owner_access ON tips.recipients TO turnesta_migrator USING (true) WITH CHECK (true);
CREATE POLICY owner_access ON tips.attempts TO turnesta_migrator USING (true) WITH CHECK (true);
CREATE POLICY manager_access ON tips.recipients TO turnesta_runtime
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
        AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = recipients.tenant_id
            AND m.user_id = nullif(current_setting('app.user_id', true), '')::uuid AND m.role IN ('owner', 'manager')))
    WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
        AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = recipients.tenant_id
            AND m.user_id = nullif(current_setting('app.user_id', true), '')::uuid AND m.role IN ('owner', 'manager')));
CREATE POLICY manager_read ON tips.attempts FOR SELECT TO turnesta_runtime
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
        AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = attempts.tenant_id
            AND m.user_id = nullif(current_setting('app.user_id', true), '')::uuid AND m.role IN ('owner', 'manager')));
GRANT SELECT, INSERT ON tips.recipients TO turnesta_runtime;
GRANT SELECT ON tips.attempts TO turnesta_runtime;

CREATE TABLE tips.mock_ledger (
    attempt_id uuid PRIMARY KEY REFERENCES tips.attempts (id),
    provider_payment_id uuid NOT NULL UNIQUE,
    recipient_id uuid NOT NULL REFERENCES tips.recipients (id),
    amount_minor bigint NOT NULL CHECK (amount_minor BETWEEN 100 AND 1000000),
    currency text NOT NULL CHECK (currency = 'RUB'),
    status text NOT NULL DEFAULT 'awaiting_payment' CHECK (status IN ('awaiting_payment', 'succeeded', 'failed', 'expired')),
    expires_at timestamptz NOT NULL,
    event_id uuid UNIQUE,
    event_payload jsonb,
    completed_at timestamptz,
    CHECK ((status = 'awaiting_payment' AND event_id IS NULL AND event_payload IS NULL AND completed_at IS NULL)
        OR (status <> 'awaiting_payment' AND event_id IS NOT NULL AND event_payload IS NOT NULL AND completed_at IS NOT NULL))
);
CREATE INDEX mock_ledger_expiry_idx ON tips.mock_ledger (expires_at) WHERE status = 'awaiting_payment';
CREATE TABLE tips.webhook_inbox (
    event_id uuid PRIMARY KEY,
    attempt_id uuid NOT NULL REFERENCES tips.attempts (id),
    payload jsonb NOT NULL,
    processed_at timestamptz NOT NULL DEFAULT pg_catalog.clock_timestamp()
);
CREATE TABLE tips.mock_delivery_outbox (
    id uuid PRIMARY KEY DEFAULT pg_catalog.gen_random_uuid(),
    event_id uuid NOT NULL UNIQUE REFERENCES tips.mock_ledger (event_id),
    event_payload jsonb NOT NULL,
    state text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'leased', 'sent', 'failed')),
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 5),
    available_at timestamptz NOT NULL DEFAULT pg_catalog.clock_timestamp(),
    lease_token uuid,
    lease_until timestamptz,
    CHECK ((state = 'leased' AND lease_token IS NOT NULL AND lease_until IS NOT NULL)
        OR (state <> 'leased' AND lease_token IS NULL AND lease_until IS NULL))
);
CREATE INDEX mock_delivery_ready_idx ON tips.mock_delivery_outbox (available_at, id) WHERE state IN ('pending', 'leased');
REVOKE ALL ON ALL TABLES IN SCHEMA tips FROM PUBLIC;
REVOKE ALL ON tips.mock_ledger, tips.webhook_inbox, tips.mock_delivery_outbox
    FROM turnesta_runtime, turnesta_provisioner, turnesta_delivery_worker;

-- The only state application path. The immutable private ledger is authoritative;
-- a correctly shaped/signed but fabricated payload is insufficient.
CREATE FUNCTION tips.apply_mock_event(p_event jsonb)
RETURNS boolean LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path = pg_catalog
AS $function$
DECLARE
    provider tips.mock_ledger%ROWTYPE;
    previous pg_catalog.jsonb;
    attempt_status pg_catalog.text;
    incoming_attempt pg_catalog.uuid;
BEGIN
    BEGIN
        incoming_attempt := (p_event->>'attempt_id')::uuid;
    EXCEPTION WHEN invalid_text_representation THEN
        RAISE EXCEPTION 'mock event does not match provider evidence' USING ERRCODE = 'T1006';
    END;
    SELECT ledger.* INTO provider FROM tips.mock_ledger ledger WHERE ledger.attempt_id = incoming_attempt FOR UPDATE;
    IF NOT FOUND OR provider.status = 'awaiting_payment' OR provider.event_payload IS DISTINCT FROM p_event THEN
        RAISE EXCEPTION 'mock event does not match provider evidence' USING ERRCODE = 'T1006';
    END IF;
    SELECT inbox.payload INTO previous FROM tips.webhook_inbox inbox WHERE inbox.event_id = provider.event_id;
    IF FOUND THEN
        IF previous IS DISTINCT FROM p_event THEN
            RAISE EXCEPTION 'mock event does not match stored inbox event' USING ERRCODE = 'T1006';
        END IF;
        RETURN false;
    END IF;
    SELECT attempt.status INTO attempt_status FROM tips.attempts attempt WHERE attempt.id = provider.attempt_id FOR UPDATE;
    IF attempt_status <> 'awaiting_payment' AND attempt_status <> provider.status THEN
        RAISE EXCEPTION 'mock attempt is already terminal' USING ERRCODE = 'T1005';
    END IF;
    UPDATE tips.attempts SET status = provider.status WHERE id = provider.attempt_id AND status = 'awaiting_payment';
    INSERT INTO tips.webhook_inbox (event_id, attempt_id, payload) VALUES (provider.event_id, provider.attempt_id, p_event);
    RETURN true;
END
$function$;

CREATE FUNCTION tips.finalize_mock_provider(p_attempt_id uuid, p_outcome text)
RETURNS jsonb LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path = pg_catalog
AS $function$
DECLARE
    provider tips.mock_ledger%ROWTYPE;
    generated_event pg_catalog.uuid;
    payload pg_catalog.jsonb;
BEGIN
    IF p_outcome IS NULL OR p_outcome NOT IN ('succeeded', 'failed', 'expired') THEN
        RAISE EXCEPTION 'invalid mock provider outcome' USING ERRCODE = 'T1005';
    END IF;
    SELECT ledger.* INTO provider FROM tips.mock_ledger ledger WHERE ledger.attempt_id = p_attempt_id FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'mock checkout not found' USING ERRCODE = 'T1002'; END IF;
    IF provider.status <> 'awaiting_payment' THEN
        IF provider.status <> p_outcome THEN RAISE EXCEPTION 'mock provider is already terminal' USING ERRCODE = 'T1005'; END IF;
        RETURN provider.event_payload;
    END IF;
    generated_event := pg_catalog.gen_random_uuid();
    payload := pg_catalog.jsonb_build_object('event_id', generated_event, 'attempt_id', provider.attempt_id,
        'provider_payment_id', provider.provider_payment_id, 'recipient_id', provider.recipient_id,
        'amount_minor', provider.amount_minor, 'currency', provider.currency, 'status', p_outcome);
    UPDATE tips.mock_ledger SET status = p_outcome, event_id = generated_event, event_payload = payload,
        completed_at = pg_catalog.clock_timestamp() WHERE attempt_id = p_attempt_id;
    INSERT INTO tips.mock_delivery_outbox (event_id, event_payload) VALUES (generated_event, payload);
    RETURN payload;
END
$function$;

CREATE FUNCTION tips.expire_mock_attempt(p_attempt_id uuid)
RETURNS void LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path = pg_catalog
AS $function$
DECLARE
    provider tips.mock_ledger%ROWTYPE;
BEGIN
    SELECT ledger.* INTO provider FROM tips.mock_ledger ledger WHERE ledger.attempt_id = p_attempt_id FOR UPDATE;
    IF FOUND AND provider.status = 'awaiting_payment' AND provider.expires_at <= pg_catalog.clock_timestamp() THEN
        PERFORM tips.apply_mock_event(tips.finalize_mock_provider(p_attempt_id, 'expired'));
    END IF;
END
$function$;

CREATE FUNCTION tips.public_recipient(p_slug uuid)
RETURNS TABLE (display_name text, location_name text, organization_name text, public_slug uuid, environment text)
LANGUAGE sql VOLATILE SECURITY DEFINER SET search_path = pg_catalog
AS $function$
    SELECT recipient.display_name, location.name, organization.name, recipient.public_slug, recipient.environment
    FROM tips.recipients recipient
    JOIN core.locations location ON location.tenant_id = recipient.tenant_id AND location.id = recipient.location_id
    JOIN core.organizations organization ON organization.id = recipient.tenant_id
    WHERE recipient.public_slug = p_slug
$function$;

CREATE FUNCTION tips.create_attempt(p_slug uuid, p_idempotency_key uuid, p_amount_minor bigint)
RETURNS TABLE (checkout_token uuid, status_token uuid)
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path = pg_catalog
AS $function$
DECLARE
    recipient tips.recipients%ROWTYPE;
    created tips.attempts%ROWTYPE;
BEGIN
    IF p_amount_minor IS NULL OR p_amount_minor NOT BETWEEN 100 AND 1000000 THEN
        RAISE EXCEPTION 'mock amount must be 100..1000000 kopecks' USING ERRCODE = 'T1001';
    END IF;
    IF p_idempotency_key IS NULL THEN RAISE EXCEPTION 'idempotency key is required' USING ERRCODE = 'T1004'; END IF;
    SELECT record.* INTO recipient FROM tips.recipients record WHERE record.public_slug = p_slug;
    IF NOT FOUND THEN RAISE EXCEPTION 'mock recipient not found' USING ERRCODE = 'T1002'; END IF;
    INSERT INTO tips.attempts (tenant_id, id, recipient_id, location_id, idempotency_key, amount_minor, checkout_token, status_token)
    VALUES (recipient.tenant_id, pg_catalog.gen_random_uuid(), recipient.id, recipient.location_id,
        p_idempotency_key, p_amount_minor, pg_catalog.gen_random_uuid(), pg_catalog.gen_random_uuid())
    ON CONFLICT (tenant_id, recipient_id, idempotency_key) DO NOTHING RETURNING * INTO created;
    IF FOUND THEN
        INSERT INTO tips.mock_ledger (attempt_id, provider_payment_id, recipient_id, amount_minor, currency, expires_at)
        VALUES (created.id, pg_catalog.gen_random_uuid(), recipient.id, created.amount_minor, 'RUB', created.expires_at);
    ELSE
        SELECT attempt.* INTO created FROM tips.attempts attempt
        WHERE attempt.tenant_id = recipient.tenant_id AND attempt.recipient_id = recipient.id AND attempt.idempotency_key = p_idempotency_key;
        IF created.amount_minor <> p_amount_minor THEN
            RAISE EXCEPTION 'idempotency key was used for a different amount' USING ERRCODE = 'T1004';
        END IF;
    END IF;
    RETURN QUERY SELECT created.checkout_token, created.status_token;
END
$function$;

CREATE FUNCTION tips.public_checkout(p_token uuid)
RETURNS TABLE (display_name text, location_name text, amount_minor bigint, currency text, status text, status_token uuid, environment text)
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path = pg_catalog
AS $function$
DECLARE matched pg_catalog.uuid;
BEGIN
    SELECT attempt.id INTO matched FROM tips.attempts attempt WHERE attempt.checkout_token = p_token;
    IF NOT FOUND THEN RETURN; END IF;
    PERFORM tips.expire_mock_attempt(matched);
    RETURN QUERY SELECT recipient.display_name, location.name, attempt.amount_minor, attempt.currency, ledger.status, attempt.status_token, attempt.environment
    FROM tips.attempts attempt JOIN tips.recipients recipient ON recipient.tenant_id = attempt.tenant_id AND recipient.id = attempt.recipient_id
    JOIN core.locations location ON location.tenant_id = attempt.tenant_id AND location.id = attempt.location_id
    JOIN tips.mock_ledger ledger ON ledger.attempt_id = attempt.id WHERE attempt.id = matched;
END
$function$;

CREATE FUNCTION tips.complete_mock_checkout(p_token uuid, p_outcome text)
RETURNS uuid LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path = pg_catalog
AS $function$
DECLARE matched tips.attempts%ROWTYPE; provider_status pg_catalog.text;
BEGIN
    IF p_outcome IS NULL OR p_outcome NOT IN ('succeeded', 'failed') THEN
        RAISE EXCEPTION 'invalid mock checkout outcome' USING ERRCODE = 'T1005';
    END IF;
    SELECT attempt.* INTO matched FROM tips.attempts attempt WHERE attempt.checkout_token = p_token;
    IF NOT FOUND THEN RAISE EXCEPTION 'mock checkout not found' USING ERRCODE = 'T1002'; END IF;
    PERFORM tips.expire_mock_attempt(matched.id);
    SELECT ledger.status INTO provider_status FROM tips.mock_ledger ledger WHERE ledger.attempt_id = matched.id FOR UPDATE;
    IF provider_status <> 'expired' THEN PERFORM tips.finalize_mock_provider(matched.id, p_outcome); END IF;
    RETURN matched.status_token;
END
$function$;

CREATE FUNCTION tips.public_attempt(p_token uuid)
RETURNS TABLE (display_name text, amount_minor bigint, currency text, status text, environment text)
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path = pg_catalog
AS $function$
DECLARE matched pg_catalog.uuid;
BEGIN
    SELECT attempt.id INTO matched FROM tips.attempts attempt WHERE attempt.status_token = p_token;
    IF NOT FOUND THEN RETURN; END IF;
    PERFORM tips.expire_mock_attempt(matched);
    RETURN QUERY SELECT recipient.display_name, attempt.amount_minor, attempt.currency, attempt.status, attempt.environment
    FROM tips.attempts attempt JOIN tips.recipients recipient ON recipient.tenant_id = attempt.tenant_id AND recipient.id = attempt.recipient_id
    WHERE attempt.id = matched;
END
$function$;

CREATE FUNCTION tips.reconcile_attempt(p_tenant_id uuid, p_attempt_id uuid)
RETURNS void LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path = pg_catalog
AS $function$
DECLARE payload pg_catalog.jsonb;
BEGIN
    IF p_tenant_id IS DISTINCT FROM nullif(current_setting('app.tenant_id', true), '')::uuid
        OR NOT EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = p_tenant_id
            AND m.user_id = nullif(current_setting('app.user_id', true), '')::uuid AND m.role IN ('owner', 'manager'))
    THEN RAISE EXCEPTION 'tips management requires owner or manager' USING ERRCODE = 'T1003'; END IF;
    IF NOT EXISTS (SELECT 1 FROM tips.attempts attempt WHERE attempt.tenant_id = p_tenant_id AND attempt.id = p_attempt_id)
    THEN RAISE EXCEPTION 'mock attempt not found' USING ERRCODE = 'T1002'; END IF;
    PERFORM tips.expire_mock_attempt(p_attempt_id);
    SELECT ledger.event_payload INTO payload FROM tips.mock_ledger ledger WHERE ledger.attempt_id = p_attempt_id;
    IF payload IS NOT NULL THEN PERFORM tips.apply_mock_event(payload); END IF;
END
$function$;

CREATE FUNCTION tips.claim_mock_delivery()
RETURNS TABLE (job_id uuid, lease_token uuid, event jsonb, attempt integer)
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path = pg_catalog ROWS 1
AS $function$
DECLARE expired_id pg_catalog.uuid;
BEGIN
    FOR expired_id IN SELECT ledger.attempt_id FROM tips.mock_ledger ledger
        WHERE ledger.status = 'awaiting_payment' AND ledger.expires_at <= pg_catalog.clock_timestamp()
        ORDER BY ledger.expires_at LIMIT 50 FOR UPDATE SKIP LOCKED
    LOOP PERFORM tips.expire_mock_attempt(expired_id); END LOOP;
    UPDATE tips.mock_delivery_outbox job SET state = 'failed', lease_token = NULL, lease_until = NULL
    WHERE job.state = 'leased' AND job.lease_until <= pg_catalog.clock_timestamp() AND job.attempts >= 5;
    RETURN QUERY WITH candidate AS (
        SELECT job.id FROM tips.mock_delivery_outbox job WHERE job.attempts < 5
            AND ((job.state = 'pending' AND job.available_at <= pg_catalog.clock_timestamp())
                OR (job.state = 'leased' AND job.lease_until <= pg_catalog.clock_timestamp()))
        ORDER BY job.available_at, job.id LIMIT 1 FOR UPDATE SKIP LOCKED
    ) UPDATE tips.mock_delivery_outbox job SET state = 'leased', attempts = job.attempts + 1,
        lease_token = pg_catalog.gen_random_uuid(), lease_until = pg_catalog.clock_timestamp() + interval '45 seconds'
    FROM candidate WHERE job.id = candidate.id RETURNING job.id, job.lease_token, job.event_payload, job.attempts;
END
$function$;

CREATE FUNCTION tips.finish_mock_delivery(p_job_id uuid, p_lease_token uuid, p_success boolean)
RETURNS boolean LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path = pg_catalog
AS $function$
DECLARE claimed tips.mock_delivery_outbox%ROWTYPE;
BEGIN
    IF p_success IS NULL THEN RAISE EXCEPTION 'delivery outcome is required' USING ERRCODE = 'T1005'; END IF;
    SELECT job.* INTO claimed FROM tips.mock_delivery_outbox job WHERE job.id = p_job_id AND job.state = 'leased'
        AND job.lease_token = p_lease_token AND job.lease_until > pg_catalog.clock_timestamp() FOR UPDATE;
    IF NOT FOUND THEN RETURN false; END IF;
    UPDATE tips.mock_delivery_outbox job SET state = CASE WHEN p_success THEN 'sent' WHEN claimed.attempts >= 5 THEN 'failed' ELSE 'pending' END,
        lease_token = NULL, lease_until = NULL,
        available_at = CASE WHEN NOT p_success THEN pg_catalog.clock_timestamp() + pg_catalog.make_interval(
            secs => CASE claimed.attempts WHEN 1 THEN 15 WHEN 2 THEN 30 WHEN 3 THEN 60 ELSE 120 END) ELSE job.available_at END
    WHERE job.id = p_job_id;
    RETURN true;
END
$function$;

REVOKE ALL ON ALL FUNCTIONS IN SCHEMA tips FROM PUBLIC, turnesta_runtime, turnesta_provisioner, turnesta_delivery_worker;
GRANT EXECUTE ON FUNCTION tips.public_recipient(uuid), tips.create_attempt(uuid, uuid, bigint), tips.public_checkout(uuid),
    tips.complete_mock_checkout(uuid, text), tips.public_attempt(uuid), tips.apply_mock_event(jsonb), tips.reconcile_attempt(uuid, uuid)
    TO turnesta_runtime;
GRANT EXECUTE ON FUNCTION tips.claim_mock_delivery(), tips.finish_mock_delivery(uuid, uuid, boolean) TO turnesta_delivery_worker;
