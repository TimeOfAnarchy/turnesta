import { afterEach, mock, test } from 'node:test';
import assert from 'node:assert/strict';
import { auditApi, formatAuditTime } from '../src/audit.ts';

afterEach(() => { mock.restoreAll(); delete globalThis.document; });

test('audit row uses the UTC filter date across local midnight', () => {
  assert.match(formatAuditTime('2026-11-17T00:30:00+03:00'), /16\.11\.2026.*21:30:00 UTC/);
});

test('audit filters and cursor stay scoped to the organization', async () => {
  const signal = new AbortController().signal;
  mock.method(globalThis, 'fetch', async (url, options) => {
    const parsed = new URL(url, 'http://localhost');
    assert.equal(parsed.pathname, '/api/organizations/tenant/audit/access');
    assert.equal(parsed.searchParams.get('actor'), 'member');
    assert.equal(parsed.searchParams.get('from'), '2026-11-01');
    assert.equal(parsed.searchParams.get('before_id'), '9223372036854775807');
    assert.equal(options.signal, signal);
    return Response.json({ items: [], next_cursor: null });
  });
  await auditApi.list('tenant', { from: '2026-11-01', to: '', actor: 'member' }, { before_at: '2026-11-17T12:00:00Z', before_id: '9223372036854775807' }, signal);
});

test('audit retention update carries the version and CSRF token', async () => {
  globalThis.document = { cookie: 'turnesta_csrf=token' };
  mock.method(globalThis, 'fetch', async (url, options) => {
    assert.equal(url, '/api/organizations/tenant/audit/settings');
    assert.equal(options.headers.get('X-CSRF-Token'), 'token');
    assert.deepEqual(JSON.parse(options.body), { retention_days: 90, expected_version: 3 });
    return Response.json({ retention_days: 90, version: 4 });
  });
  assert.equal((await auditApi.configure('tenant', 90, 3)).version, 4);
});
