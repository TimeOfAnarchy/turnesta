import assert from 'node:assert/strict';
import { afterEach, mock, test } from 'node:test';
import { api, ApiError, errorMessage } from '../src/api.ts';
import { formatTipAmount, formatTipDate } from '../src/tipsFormat.ts';

afterEach(() => {
  mock.restoreAll();
  delete globalThis.document;
});

test('recipient and payment lists stay scoped to their organization, cursor and cancellation signal', async () => {
  const controller = new AbortController();
  const calls = [];
  mock.method(globalThis, 'fetch', async (url, options) => {
    calls.push(url);
    assert.equal(options.credentials, 'same-origin');
    assert.equal(options.signal, controller.signal);
    assert.equal(options.headers.has('X-CSRF-Token'), false);
    return Response.json({ items: [], next_cursor: null });
  });
  await api.tipRecipients('tenant-one', null, controller.signal);
  await api.tipAttempts('tenant-two', 'next-cursor', controller.signal);
  assert.deepEqual(calls, [
    '/api/organizations/tenant-one/tips/recipients?limit=50',
    '/api/organizations/tenant-two/tips/attempts?limit=50&after=next-cursor',
  ]);
});

test('creating a recipient sends only display name and location, with CSRF', async () => {
  globalThis.document = { cookie: 'turnesta_csrf=csrf-value' };
  mock.method(globalThis, 'fetch', async (url, options) => {
    assert.equal(url, '/api/organizations/tenant-one/tips/recipients');
    assert.equal(options.method, 'POST');
    assert.equal(options.headers.get('X-CSRF-Token'), 'csrf-value');
    assert.equal(options.headers.get('Content-Type'), 'application/json');
    assert.deepEqual(JSON.parse(options.body), { display_name: 'Анна', location_id: 'existing-location' });
    return Response.json({ id: 'recipient', display_name: 'Анна', location_id: 'existing-location', tenant_id: 'tenant-one', public_slug: 'public-slug', environment: 'mock' }, { status: 201 });
  });
  const created = await api.createTipRecipient('tenant-one', { display_name: 'Анна', location_id: 'existing-location' });
  assert.equal(created.environment, 'mock');
  assert.equal(created.public_slug, 'public-slug');
});

test('reconcile requests the server fact with no client outcome or request body', async () => {
  globalThis.document = { cookie: '__Host-turnesta_csrf=secure-csrf' };
  mock.method(globalThis, 'fetch', async (url, options) => {
    assert.equal(url, '/api/organizations/tenant-one/tips/attempts/attempt-id/reconcile');
    assert.equal(options.method, 'POST');
    assert.equal(options.credentials, 'same-origin');
    assert.equal(options.headers.get('X-CSRF-Token'), 'secure-csrf');
    assert.equal(options.body, undefined);
    assert.equal(options.headers.has('Content-Type'), false);
    return Response.json({ id: 'attempt-id', status: 'awaiting_payment' });
  });
  const result = await api.reconcileTipAttempt('tenant-one', 'attempt-id');
  assert.equal(result.status, 'awaiting_payment');
});

test('disabled tips is an explicit error, without an automatic mutation replay', async () => {
  globalThis.document = { cookie: 'turnesta_csrf=value' };
  let calls = 0;
  mock.method(globalThis, 'fetch', async () => {
    calls += 1;
    return Response.json({ code: 'tips_disabled' }, { status: 503 });
  });
  await assert.rejects(api.reconcileTipAttempt('tenant', 'attempt'), (error) => error instanceof ApiError && error.code === 'tips_disabled' && errorMessage(error).includes('выключен'));
  assert.equal(calls, 1);
});

test('RUB minor amounts render precisely at the smallest and largest supported bounds', () => {
  const spaces = (value) => value.replace(/\s/gu, ' ');
  assert.equal(spaces(formatTipAmount(1, 'RUB')), '0,01 ₽');
  assert.equal(spaces(formatTipAmount(101, 'RUB')), '1,01 ₽');
  assert.equal(spaces(formatTipAmount(1_000_000, 'RUB')), '10 000,00 ₽');
});

test('invalid amounts and unexpected currencies cannot appear as believable money', () => {
  for (const amount of [0, -100, 1.1, NaN, Infinity, 1_000_001, Number.MAX_SAFE_INTEGER + 1]) {
    assert.equal(formatTipAmount(amount, 'RUB'), 'Сумма недоступна');
  }
  assert.equal(formatTipAmount(500, 'USD'), 'Сумма недоступна');
});

test('invalid provider timestamps have a human-readable fallback', () => {
  assert.equal(formatTipDate('not-a-timestamp'), 'Дата недоступна');
});
