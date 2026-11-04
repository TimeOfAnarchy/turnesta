ALTER TABLE bookings.public_settings ADD COLUMN privacy_url text
    CHECK(privacy_url IS NULL OR (char_length(privacy_url)<=500 AND privacy_url ~ '^https://[^[:space:]]+$'));
UPDATE bookings.public_settings SET enabled=false,version=version+1 WHERE enabled;
ALTER TABLE bookings.public_settings ADD CONSTRAINT public_booking_requires_privacy CHECK(NOT enabled OR privacy_url IS NOT NULL);
GRANT UPDATE(privacy_url) ON bookings.public_settings TO turnesta_runtime;

ALTER TABLE bookings.appointments ADD COLUMN consent_url text;
ALTER TABLE bookings.appointments ADD COLUMN consent_at timestamptz;
ALTER TABLE bookings.appointments ADD CONSTRAINT appointment_consent_pair CHECK((consent_url IS NULL)=(consent_at IS NULL));

CREATE OR REPLACE FUNCTION bookings.public_profile(p_slug uuid) RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $f$
SELECT jsonb_build_object('public_slug',s.public_slug,'organization_name',o.name,'location_name',l.name,'time_zone',l.time_zone,
    'today',to_char(current_timestamp AT TIME ZONE l.time_zone,'YYYY-MM-DD'),'enabled',s.enabled,'version',s.version,
    'privacy_url',CASE WHEN s.enabled THEN s.privacy_url ELSE NULL END,
    'form',CASE WHEN s.enabled THEN s.form ELSE NULL END,'specialists',CASE WHEN s.enabled THEN coalesce((SELECT jsonb_agg(jsonb_build_object('id',p.id,'name',p.name,'description',p.description,'version',p.version) ORDER BY p.name,p.id)
        FROM bookings.specialists p WHERE p.tenant_id=s.tenant_id AND p.location_id=s.location_id AND p.enabled),'[]') ELSE '[]'::jsonb END)
FROM bookings.public_settings s JOIN core.locations l ON l.tenant_id=s.tenant_id AND l.id=s.location_id JOIN core.organizations o ON o.id=s.tenant_id WHERE s.public_slug=p_slug
$f$;

DROP FUNCTION bookings.public_book(uuid,bytea,uuid,integer,integer,integer,text,jsonb);
CREATE FUNCTION bookings.public_book(p_slug uuid,p_hash bytea,p_offer uuid,p_offer_version integer,p_specialist_version integer,p_form_version integer,p_zone text,p_answers jsonb,p_consent boolean)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE settings bookings.public_settings%ROWTYPE; target bookings.offers%ROWTYPE; fingerprint bytea; normalized jsonb; previous bytea; appointment uuid:=gen_random_uuid(); point_zone text;
BEGIN
    IF octet_length(p_hash) IS DISTINCT FROM 32 THEN RAISE EXCEPTION 'invalid access' USING ERRCODE='B2204'; END IF;
    normalized:=bookings.clean_answers(p_answers);
    IF p_offer IS NULL OR p_offer_version IS NULL OR p_offer_version<1 OR p_specialist_version IS NULL OR p_specialist_version<1 OR p_form_version IS NULL OR p_form_version<1 OR p_zone IS NULL THEN RAISE EXCEPTION 'invalid booking' USING ERRCODE='B2204'; END IF;
    fingerprint:=sha256(convert_to(jsonb_build_object('offer_id',p_offer,'offer_version',p_offer_version,'specialist_version',p_specialist_version,'form_version',p_form_version,'time_zone',p_zone,'answers',normalized)::text,'UTF8'));
    SELECT * INTO settings FROM bookings.public_settings WHERE public_slug=p_slug FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'booking page not found' USING ERRCODE='B2201'; END IF;
    SELECT request_fingerprint INTO previous FROM bookings.public_claims WHERE tenant_id=settings.tenant_id AND location_id=settings.location_id AND token_hash=p_hash;
    IF FOUND THEN
        IF previous<>fingerprint THEN RAISE EXCEPTION 'different request' USING ERRCODE='B2206'; END IF;
        RETURN bookings.public_receipt(p_slug,p_hash);
    END IF;
    IF NOT settings.enabled THEN RAISE EXCEPTION 'booking paused' USING ERRCODE='B2202'; END IF;
    SELECT time_zone INTO point_zone FROM core.locations WHERE tenant_id=settings.tenant_id AND id=settings.location_id;
    IF settings.version<>p_form_version OR point_zone<>p_zone THEN RAISE EXCEPTION 'review changed form or timezone' USING ERRCODE='B2205'; END IF;
    IF p_consent IS NOT TRUE THEN RAISE EXCEPTION 'consent required' USING ERRCODE='B2204'; END IF;
    PERFORM bookings.validate_answers(settings.form,normalized);
    SELECT * INTO target FROM bookings.offers WHERE tenant_id=settings.tenant_id AND location_id=settings.location_id AND id=p_offer;
    IF NOT FOUND THEN RAISE EXCEPTION 'offer unavailable' USING ERRCODE='B2103'; END IF;
    PERFORM 1 FROM bookings.calendars WHERE tenant_id=target.tenant_id AND location_id=target.location_id AND specialist_id=target.specialist_id FOR UPDATE;
    SELECT * INTO target FROM bookings.offers WHERE tenant_id=settings.tenant_id AND location_id=settings.location_id AND id=p_offer;
    IF target.version<>p_offer_version OR NOT EXISTS(SELECT 1 FROM bookings.specialists WHERE tenant_id=target.tenant_id AND id=target.specialist_id AND version=p_specialist_version AND enabled) THEN RAISE EXCEPTION 'offer or specialist changed' USING ERRCODE='B2205'; END IF;
    IF target.state<>'ready' OR (target.starts_at IS NOT NULL AND target.starts_at<=clock_timestamp())
        OR (target.starts_at IS NULL AND target.local_date IS NOT NULL AND target.local_date<(clock_timestamp() AT TIME ZONE point_zone)::date)
        OR coalesce((target.starts_at AT TIME ZONE point_zone)::date,target.local_date)>=(date_trunc('month',clock_timestamp() AT TIME ZONE point_zone)+interval '13 months')::date
    THEN RAISE EXCEPTION 'offer unavailable' USING ERRCODE='B2103'; END IF;
    IF (SELECT count(*) FROM bookings.public_claims WHERE tenant_id=settings.tenant_id AND location_id=settings.location_id AND created_at>clock_timestamp()-interval '1 minute')>=30
        OR (SELECT count(*) FROM bookings.public_claims c JOIN bookings.appointments a ON a.tenant_id=c.tenant_id AND a.id=c.appointment_id WHERE c.tenant_id=settings.tenant_id AND c.location_id=settings.location_id AND a.status='confirmed')>=1000
    THEN RAISE EXCEPTION 'booking rate exceeded' USING ERRCODE='B2203'; END IF;
    INSERT INTO bookings.appointments(tenant_id,location_id,specialist_id,offer_id,id,client_name,phone,email,note,status,source,form_snapshot,answers,consent_url,consent_at)
        VALUES(target.tenant_id,target.location_id,target.specialist_id,target.id,appointment,normalized->>'client_name',coalesce(normalized->>'phone',''),coalesce(normalized->>'email',''),coalesce(normalized->>'note',''),'confirmed','public',settings.form,normalized,settings.privacy_url,clock_timestamp());
    INSERT INTO bookings.public_claims(tenant_id,location_id,token_hash,appointment_id,request_fingerprint,public_zone)
        VALUES(target.tenant_id,target.location_id,p_hash,appointment,fingerprint,point_zone);
    RETURN bookings.public_receipt(p_slug,p_hash);
END $f$;
REVOKE ALL ON FUNCTION bookings.public_book(uuid,bytea,uuid,integer,integer,integer,text,jsonb,boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION bookings.public_book(uuid,bytea,uuid,integer,integer,integer,text,jsonb,boolean) TO turnesta_runtime;
