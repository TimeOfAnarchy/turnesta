ALTER TABLE core.organizations ADD COLUMN version integer NOT NULL DEFAULT 1 CHECK (version > 0);
ALTER TABLE core.locations ADD COLUMN version integer NOT NULL DEFAULT 1 CHECK (version > 0);

GRANT UPDATE (name, version) ON core.organizations TO turnesta_runtime;
GRANT UPDATE (name, time_zone, version) ON core.locations TO turnesta_runtime;

CREATE POLICY tenant_update ON core.organizations FOR UPDATE TO turnesta_runtime
    USING (
        id = nullif(current_setting('app.tenant_id', true), '')::uuid
        AND EXISTS (
            SELECT 1 FROM core.memberships m
            WHERE m.tenant_id = organizations.id AND m.role IN ('owner', 'manager')
        )
    ) WITH CHECK (
        id = nullif(current_setting('app.tenant_id', true), '')::uuid
        AND EXISTS (
            SELECT 1 FROM core.memberships m
            WHERE m.tenant_id = organizations.id AND m.role IN ('owner', 'manager')
        )
    );

CREATE POLICY tenant_update ON core.locations FOR UPDATE TO turnesta_runtime
    USING (
        tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
        AND EXISTS (
            SELECT 1 FROM core.memberships m
            WHERE m.tenant_id = locations.tenant_id AND m.role IN ('owner', 'manager')
        )
    ) WITH CHECK (
        tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
        AND EXISTS (
            SELECT 1 FROM core.memberships m
            WHERE m.tenant_id = locations.tenant_id AND m.role IN ('owner', 'manager')
        )
    );
