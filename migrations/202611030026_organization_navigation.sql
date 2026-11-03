-- Presentation preference only; publication and authorization remain independent.
ALTER TABLE core.organizations ADD COLUMN visible_services text[] NOT NULL
    DEFAULT ARRAY['queues','bookings','tips']::text[];
ALTER TABLE core.organizations ADD CONSTRAINT organization_visible_services_valid CHECK (
    visible_services <@ ARRAY['queues','bookings','tips']::text[]
    AND array_position(visible_services,NULL) IS NULL
    AND cardinality(visible_services) =
        ('queues'=ANY(visible_services))::integer +
        ('bookings'=ANY(visible_services))::integer +
        ('tips'=ANY(visible_services))::integer
);
GRANT UPDATE(visible_services) ON core.organizations TO turnesta_runtime;
