CREATE FUNCTION auth.password_budget_check(p_email text,p_scope text,p_client text) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE issued timestamptz:=clock_timestamp();
BEGIN
    IF p_scope IS NULL OR p_scope NOT IN('login','change','setup') OR p_email IS NULL OR octet_length(p_email)>254
        OR p_client IS NULL OR octet_length(p_client)>255 OR p_client=''
    THEN RETURN false; END IF;
    RETURN NOT EXISTS(
        SELECT 1 FROM auth.password_attempt_windows w
        JOIN (VALUES
            (sha256(convert_to('turnesta.password.client_email:'||jsonb_build_array(p_scope,p_client,p_email)::text,'UTF8')),interval '15 minutes',10),
            (sha256(convert_to('turnesta.password.client:'||jsonb_build_array(p_scope,p_client)::text,'UTF8')),interval '15 minutes',50),
            (sha256(convert_to('turnesta.password.email_hour:'||jsonb_build_array(p_scope,p_email)::text,'UTF8')),interval '1 hour',100)
        ) AS budget(bucket,duration,maximum) ON budget.bucket=w.bucket
        WHERE w.window_start>issued-budget.duration AND w.attempts>=budget.maximum
    );
END $f$;

CREATE FUNCTION auth.password_failure(p_email text,p_scope text,p_client text) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $f$
DECLARE issued timestamptz:=clock_timestamp(); item record; count_now integer; alert_count integer;
BEGIN
    IF p_scope IS NULL OR p_scope NOT IN('login','change','setup') OR p_email IS NULL OR octet_length(p_email)>254
        OR p_client IS NULL OR octet_length(p_client)>255 OR p_client=''
    THEN RAISE EXCEPTION 'invalid password budget input' USING ERRCODE='22023'; END IF;
    FOR item IN SELECT * FROM (VALUES
        (1,sha256(convert_to('turnesta.password.client_email:'||jsonb_build_array(p_scope,p_client,p_email)::text,'UTF8')),interval '15 minutes'),
        (2,sha256(convert_to('turnesta.password.client:'||jsonb_build_array(p_scope,p_client)::text,'UTF8')),interval '15 minutes'),
        (3,sha256(convert_to('turnesta.password.email_hour:'||jsonb_build_array(p_scope,p_email)::text,'UTF8')),interval '1 hour'),
        (4,sha256(convert_to('turnesta.password.alert:'||p_scope,'UTF8')),interval '5 minutes')
    ) AS budgets(position,bucket,duration) ORDER BY position LOOP
        INSERT INTO auth.password_attempt_windows AS w VALUES(item.bucket,issued,1)
        ON CONFLICT(bucket) DO UPDATE SET
            window_start=CASE WHEN w.window_start<=issued-item.duration THEN issued ELSE w.window_start END,
            attempts=CASE WHEN w.window_start<=issued-item.duration THEN 1 ELSE least(w.attempts+1,1000000) END
        RETURNING attempts INTO count_now;
        IF item.position=4 THEN alert_count:=count_now; END IF;
    END LOOP;
    IF alert_count=1 OR alert_count%200=0 THEN
        DELETE FROM auth.password_attempt_windows WHERE bucket IN(
            SELECT bucket FROM auth.password_attempt_windows WHERE window_start<issued-interval '1 day'
            LIMIT 1000 FOR UPDATE SKIP LOCKED);
    END IF;
    RETURN alert_count>=200 AND alert_count%200=0;
END $f$;

REVOKE ALL ON FUNCTION auth.password_budget(text,text),auth.password_budget_check(text,text,text),auth.password_failure(text,text,text) FROM PUBLIC,turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker;
GRANT EXECUTE ON FUNCTION auth.password_budget_check(text,text,text),auth.password_failure(text,text,text) TO turnesta_runtime;
