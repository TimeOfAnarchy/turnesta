CREATE TABLE core.time_zones (
    name text PRIMARY KEY
);
INSERT INTO core.time_zones (name)
SELECT name FROM pg_timezone_names WHERE name !~ '^(posix|right)/';

CREATE TABLE core.users (
    id uuid PRIMARY KEY,
    display_name text NOT NULL CHECK (
        char_length(display_name) BETWEEN 1 AND 120
        AND display_name = btrim(display_name)
        AND display_name !~ '[[:cntrl:]]'
    ),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE core.organizations (
    id uuid PRIMARY KEY,
    name text NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 120
        AND name = btrim(name)
        AND name !~ '[[:cntrl:]]'
    ),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE core.memberships (
    tenant_id uuid NOT NULL REFERENCES core.organizations (id),
    user_id uuid NOT NULL REFERENCES core.users (id),
    role text NOT NULL CHECK (role IN ('owner', 'manager', 'operator', 'employee')),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, user_id)
);
CREATE INDEX memberships_user_id_idx ON core.memberships (user_id, tenant_id);

CREATE TABLE core.locations (
    tenant_id uuid NOT NULL REFERENCES core.organizations (id),
    id uuid NOT NULL,
    name text NOT NULL CHECK (
        char_length(name) BETWEEN 1 AND 120
        AND name = btrim(name)
        AND name !~ '[[:cntrl:]]'
    ),
    time_zone text NOT NULL REFERENCES core.time_zones (name),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, id)
);

REVOKE ALL ON SCHEMA core FROM PUBLIC;
REVOKE ALL ON ALL TABLES IN SCHEMA core FROM PUBLIC;
GRANT USAGE ON SCHEMA core TO turnesta_runtime, turnesta_provisioner;
GRANT SELECT ON core.time_zones TO turnesta_runtime, turnesta_provisioner;
GRANT SELECT ON core.users, core.organizations, core.memberships TO turnesta_runtime;
GRANT SELECT, INSERT ON core.locations TO turnesta_runtime;
GRANT SELECT, INSERT ON core.users, core.organizations, core.memberships, core.locations
    TO turnesta_provisioner;

ALTER TABLE core.users ENABLE ROW LEVEL SECURITY;
ALTER TABLE core.users FORCE ROW LEVEL SECURITY;
ALTER TABLE core.organizations ENABLE ROW LEVEL SECURITY;
ALTER TABLE core.organizations FORCE ROW LEVEL SECURITY;
ALTER TABLE core.memberships ENABLE ROW LEVEL SECURITY;
ALTER TABLE core.memberships FORCE ROW LEVEL SECURITY;
ALTER TABLE core.locations ENABLE ROW LEVEL SECURITY;
ALTER TABLE core.locations FORCE ROW LEVEL SECURITY;

CREATE POLICY provisioner ON core.users TO turnesta_provisioner
    USING (true) WITH CHECK (true);
CREATE POLICY provisioner ON core.organizations TO turnesta_provisioner
    USING (true) WITH CHECK (true);
CREATE POLICY provisioner ON core.memberships TO turnesta_provisioner
    USING (true) WITH CHECK (true);
CREATE POLICY provisioner ON core.locations TO turnesta_provisioner
    USING (true) WITH CHECK (true);

CREATE POLICY self_read ON core.users FOR SELECT TO turnesta_runtime
    USING (id = nullif(current_setting('app.user_id', true), '')::uuid);

CREATE POLICY own_membership ON core.memberships FOR SELECT TO turnesta_runtime
    USING (
        tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
        AND user_id = nullif(current_setting('app.user_id', true), '')::uuid
    );

CREATE POLICY tenant_read ON core.organizations FOR SELECT TO turnesta_runtime
    USING (
        id = nullif(current_setting('app.tenant_id', true), '')::uuid
        AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = id)
    );

CREATE POLICY tenant_read ON core.locations FOR SELECT TO turnesta_runtime
    USING (
        tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
        AND EXISTS (SELECT 1 FROM core.memberships m WHERE m.tenant_id = locations.tenant_id)
    );
CREATE POLICY tenant_insert ON core.locations FOR INSERT TO turnesta_runtime
    WITH CHECK (
        tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid
        AND EXISTS (
            SELECT 1 FROM core.memberships m
            WHERE m.tenant_id = locations.tenant_id AND m.role IN ('owner', 'manager')
        )
    );
