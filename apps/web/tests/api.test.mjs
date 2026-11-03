import assert from 'node:assert/strict';
import { afterEach, mock, test } from 'node:test';
import { api, ApiError, errorMessage, onSignedOut, readRecoveryInterval, validateName } from '../src/api.ts';

afterEach(() => {
  mock.restoreAll();
  delete globalThis.document;
});

test('navigation save sends tenant choice and version once with CSRF and a deadline', async () => {
  globalThis.document = { cookie: 'turnesta_csrf=token' };
  let calls = 0;
  mock.method(globalThis, 'fetch', async (url, options) => {
    calls++;
    assert.equal(url, '/api/organizations/tenant-id/navigation');
    assert.equal(options.method, 'PUT');
    assert.equal(options.headers.get('X-CSRF-Token'), 'token');
    assert.deepEqual(JSON.parse(options.body), {visible_services:['bookings'],expected_version:4});
    assert.ok(options.signal instanceof AbortSignal);
    return Response.json({ code:'version_conflict' }, { status:409 });
  });
  await assert.rejects(api.setNavigation('tenant-id', ['bookings'], 4), error => error instanceof ApiError && error.code === 'version_conflict');
  assert.equal(calls, 1);
});

test('read recovery retries transient failures and stops for access and validation errors', () => {
  assert.equal(readRecoveryInterval(null), false);
  assert.equal(readRecoveryInterval(new TypeError('network')), 5_000);
  for (const status of [408,429,500,503]) assert.equal(readRecoveryInterval(new ApiError(status,'failure')),5_000);
  for (const status of [400,401,403,404,409,410]) assert.equal(readRecoveryInterval(new ApiError(status,'failure')),false);
});

test('names validate Unicode characters, control characters and bounds', () => {
  assert.equal(validateName('  На Лесной  '), undefined);
  assert.equal(validateName('🙂'.repeat(120)), undefined);
  assert.ok(validateName('🙂'.repeat(121)));
  assert.ok(validateName('    '));
  assert.ok(validateName('Точка\u0000центр'));
});

test('read requests use same-origin credentials and the query cancellation signal', async () => {
  const controller = new AbortController();
  mock.method(globalThis, 'fetch', async (url, options) => {
    assert.equal(url, '/api/organizations/tenant-id/locations?limit=50&after=cursor');
    assert.equal(options.credentials, 'same-origin');
    assert.equal(options.signal, controller.signal);
    assert.equal(options.headers.has('X-CSRF-Token'), false);
    return Response.json({ items: [], next_cursor: null });
  });
  assert.deepEqual(await api.locations('tenant-id', 'cursor', controller.signal), { items: [], next_cursor: null });
});

test('mutation sends the version and readable CSRF token without reading session cookies', async () => {
  globalThis.document = { cookie: 'irrelevant=value; turnesta_csrf=csrf-token' };
  mock.method(globalThis, 'fetch', async (url, options) => {
    assert.equal(url, '/api/organizations/tenant-id/locations/location-id');
    assert.equal(options.method, 'PATCH');
    assert.equal(options.credentials, 'same-origin');
    assert.equal(options.headers.get('X-CSRF-Token'), 'csrf-token');
    assert.equal(options.headers.get('Content-Type'), 'application/json');
    assert.deepEqual(JSON.parse(options.body), { name: 'На Лесной', time_zone: 'Europe/Moscow', expected_version: 7 });
    return Response.json({ id: 'location-id', tenant_id: 'tenant-id', name: 'На Лесной', time_zone: 'Europe/Moscow', version: 8 });
  });
  assert.equal((await api.updateLocation('tenant-id', 'location-id', { name: 'На Лесной', time_zone: 'Europe/Moscow' }, 7)).version, 8);
});

test('version conflict retains the server error code and has no automatic request retry', async () => {
  globalThis.document = { cookie: '__Host-turnesta_csrf=secure-csrf; turnesta_csrf=legacy-csrf' };
  let calls = 0;
  mock.method(globalThis, 'fetch', async (_url, options) => {
    calls += 1;
    assert.equal(options.headers.get('X-CSRF-Token'), 'secure-csrf');
    return Response.json({ code: 'version_conflict', message: 'server detail' }, { status: 409 });
  });
  await assert.rejects(api.renameOrganization('tenant-id', 'Название', 2), (error) => error instanceof ApiError && error.status === 409 && error.code === 'version_conflict');
  assert.equal(calls, 1);
});

test('unauthorized response invokes sign-out before rejecting the request', async () => {
  let redirected = false;
  onSignedOut(() => { redirected = true; });
  mock.method(globalThis, 'fetch', async () => new Response(null, { status: 401 }));
  await assert.rejects(api.session(new AbortController().signal), (error) => redirected && error instanceof ApiError && error.status === 401);
});

test('user-facing errors do not echo arbitrary server data', () => {
  const text = errorMessage(new ApiError(500, '<sensitive-server-data>'));
  assert.equal(text.includes('sensitive'), false);
  assert.ok(text.includes('недоступен'));
});
