CREATE FUNCTION bookings.widget_origins_valid(origins text[]) RETURNS boolean
LANGUAGE sql IMMUTABLE SECURITY INVOKER SET search_path=pg_catalog AS $f$
    SELECT origins IS NOT NULL AND coalesce(array_ndims(origins),1)=1
        AND cardinality(origins)<=10
        AND NOT EXISTS (
            SELECT 1 FROM unnest(origins) origin WHERE origin IS NULL
                OR octet_length(origin)>255
                OR origin !~ '^(https://([a-z0-9]([a-z0-9.-]*[a-z0-9])?|\[[0-9a-f:]+\])|http://(localhost|127\.0\.0\.1|\[::1\]))(:[1-9][0-9]{0,4})?$'
        )
        AND cardinality(origins)=(SELECT count(DISTINCT origin) FROM unnest(origins) origin)
$f$;

CREATE TABLE bookings.widget_settings (
    tenant_id uuid NOT NULL,
    location_id uuid NOT NULL,
    allowed_origins text[] NOT NULL CHECK(bookings.widget_origins_valid(allowed_origins)),
    version integer NOT NULL DEFAULT 1 CHECK(version>0),
    PRIMARY KEY(tenant_id,location_id),
    FOREIGN KEY(tenant_id,location_id) REFERENCES core.locations(tenant_id,id)
);
ALTER TABLE bookings.widget_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE bookings.widget_settings FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_read ON bookings.widget_settings FOR SELECT TO turnesta_runtime USING (
    tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
    AND EXISTS(SELECT 1 FROM core.memberships m WHERE m.tenant_id=widget_settings.tenant_id
        AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid));
CREATE POLICY manager_insert ON bookings.widget_settings FOR INSERT TO turnesta_runtime WITH CHECK (
    tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
    AND EXISTS(SELECT 1 FROM core.memberships m WHERE m.tenant_id=widget_settings.tenant_id
        AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.role IN ('owner','manager')));
CREATE POLICY manager_update ON bookings.widget_settings FOR UPDATE TO turnesta_runtime USING (
    tenant_id=nullif(current_setting('app.tenant_id',true),'')::uuid
    AND EXISTS(SELECT 1 FROM core.memberships m WHERE m.tenant_id=widget_settings.tenant_id
        AND m.user_id=nullif(current_setting('app.user_id',true),'')::uuid AND m.role IN ('owner','manager')));
CREATE POLICY widget_owner_read ON bookings.widget_settings FOR SELECT TO CURRENT_USER USING(true);
GRANT SELECT,INSERT ON bookings.widget_settings TO turnesta_runtime;
GRANT UPDATE(allowed_origins,version) ON bookings.widget_settings TO turnesta_runtime;

CREATE FUNCTION bookings.widget_origins(p_slug uuid) RETURNS text[]
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $f$
    SELECT coalesce((SELECT w.allowed_origins FROM bookings.public_settings p
        JOIN bookings.widget_settings w USING(tenant_id,location_id)
        WHERE p.public_slug=p_slug),ARRAY[]::text[])
$f$;
REVOKE ALL ON FUNCTION bookings.widget_origins_valid(text[]) FROM PUBLIC;
REVOKE ALL ON FUNCTION bookings.widget_origins(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION bookings.widget_origins_valid(text[]) TO turnesta_runtime;
GRANT EXECUTE ON FUNCTION bookings.widget_origins(uuid) TO turnesta_runtime;
