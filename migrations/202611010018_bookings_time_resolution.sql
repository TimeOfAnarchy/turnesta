-- Local time resolution now uses Jiff's transition tables in the API.
-- Retain the previously applied function but remove it from the runtime surface.
REVOKE EXECUTE ON FUNCTION bookings.resolve_local(text,text) FROM turnesta_runtime;
