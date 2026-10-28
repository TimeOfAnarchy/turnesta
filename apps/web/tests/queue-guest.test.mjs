import assert from 'node:assert/strict';
import { afterEach, mock, test } from 'node:test';
import { api } from '../src/api.ts';
import { GuestError, GuestTicketClient, validTicket } from '../../api/web/queue-client.mjs';

afterEach(() => { mock.restoreAll(); delete globalThis.document; });
const slug = '12be86da-ec57-48ba-b831-05a4430737d4';
const secretA = 'ab'.repeat(32), secretB = 'cd'.repeat(32);
const waiting = { number: 7, service_date: '2026-10-28', status: 'waiting', issued_at: '2026-10-28T12:00:00.000Z', people_ahead: 1 };
function storage() { const values = new Map(); return { getItem: (key) => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) }; }
function locks() {
  let previous = Promise.resolve();
  return { request: (_key, callback) => { const result = previous.then(callback); previous = result.catch(() => {}); return result; } };
}

test('credential is stored before issuance; lost response and reload never auto-issue', async () => {
  const saved = storage(), lock = locks(); const requests = [];
  const first = new GuestTicketClient(slug, saved, lock, async (method, token) => {
    assert.equal(saved.getItem(first.storageKey), token);
    requests.push([method, token]); throw new TypeError('lost response');
  }, () => secretA);
  await assert.rejects(first.join());
  const restored = new GuestTicketClient(slug, saved, lock, async (method, token) => {
    requests.push([method, token]);
    if (method === 'GET') throw new GuestError(404, 'not_found');
    return waiting;
  }, () => secretB);
  assert.equal(requests.length, 1);
  await assert.rejects(restored.refresh());
  assert.deepEqual(await restored.join(), waiting);
  assert.deepEqual(requests, [['POST', secretA], ['GET', secretA], ['POST', secretA]]);
});

test('tabs share one credential and existing-ticket reads are independent of publication state', async () => {
  const saved = storage(), lock = locks(); const tokens = [];
  const send = async (_method, token) => { tokens.push(token); return waiting; };
  const a = new GuestTicketClient(slug, saved, lock, send, () => secretA);
  const b = new GuestTicketClient(slug, saved, lock, send, () => secretB);
  const [first, second] = await Promise.all([a.join(), b.join()]);
  assert.deepEqual(first, second); assert.deepEqual(tokens, [secretA, secretA]);
  assert.deepEqual(await b.refresh(), waiting);
});

test('only an explicit new visit after a terminal ticket changes the credential', async () => {
  const saved = storage(), lock = locks(); let generation = 0; const sent = [];
  const client = new GuestTicketClient(slug, saved, lock, async (method, token) => {
    sent.push([method, token]); return waiting;
  }, () => generation++ ? secretB : secretA);
  await client.join();
  await assert.rejects(client.join(true), (error) => error.code === 'active_ticket');
  client.ticket = { ...waiting, status: 'completed', people_ahead: null };
  await client.join(true);
  assert.deepEqual(sent, [['POST', secretA], ['POST', secretB]]);
});

test('a new-visit action queued behind another tab follows its new credential', async () => {
  const saved = storage(); const key = `turnesta:guest-ticket:v1:${slug}`; saved.setItem(key, secretA);
  let resume;
  const lock = { request: (_name, callback) => new Promise((resolve) => { resume = async () => resolve(await callback()); }) };
  const sent = [];
  const client = new GuestTicketClient(slug, saved, lock, async (method, token) => { sent.push([method, token]); return waiting; }, () => 'ef'.repeat(32));
  client.ticket = { ...waiting, status: 'completed', people_ahead: null };
  const pending = client.join(true);
  // Another tab creates a new visit while this tab waits for its browser lock.
  saved.setItem(key, secretB);
  await client.refresh();
  await resume(); await pending;
  assert.equal(saved.getItem(key), secretB);
  assert.deepEqual(sent, [['GET', secretB], ['POST', secretB]]);
});

test('late old-ticket responses cannot overwrite a new active ticket or expire its access', async () => {
  for (const expires of [false, true]) {
    const saved = storage(); const key = `turnesta:guest-ticket:v1:${slug}`; saved.setItem(key, secretA);
    let release;
    const client = new GuestTicketClient(slug, saved, locks(), (method) => {
      if (method === 'GET') return new Promise((resolve, reject) => { release = () => expires ? reject(new GuestError(410, 'ticket_expired')) : resolve({ ...waiting, status: 'completed', people_ahead: null }); });
      return Promise.resolve(waiting);
    }, () => secretB);
    client.ticket = { ...waiting, status: 'completed', people_ahead: null };
    const oldRead = client.refresh();
    await client.join(true); release(); await oldRead;
    assert.equal(client.token, secretB); assert.deepEqual(client.ticket, waiting); assert.equal(client.expired, false);
  }
});

test('rate, closed, network and malformed-success errors preserve the same credential', async () => {
  for (const failure of [new GuestError(409, 'queue_closed'), new GuestError(429, 'queue_rate_limit'), new TypeError('network'), null]) {
    const saved = storage(); const sent = [];
    const client = new GuestTicketClient(slug, saved, locks(), async (_method, token) => {
      sent.push(token); if (failure) throw failure; return { number: 'bad' };
    }, () => secretA);
    await assert.rejects(client.join()); await assert.rejects(client.join());
    assert.deepEqual(sent, [secretA, secretA]); assert.equal(saved.getItem(client.storageKey), secretA);
  }
});

test('expiry enables a new visit; missing storage or browser locks never sends a request', async () => {
  const saved = storage(); saved.setItem(`turnesta:guest-ticket:v1:${slug}`, secretA);
  const client = new GuestTicketClient(slug, saved, locks(), async () => { throw new GuestError(410, 'ticket_expired'); });
  await assert.rejects(client.refresh()); assert.equal(client.canStartNew(), true);
  assert.throws(() => new GuestTicketClient(slug, null, locks()), (error) => error.code === 'storage_unavailable');
  const brokenStorage = { getItem: () => null, setItem: () => { throw new Error('quota'); } };
  let sent = 0;
  const broken = new GuestTicketClient(slug, brokenStorage, locks(), async () => { sent++; return waiting; });
  await assert.rejects(broken.join(), (error) => error.code === 'storage_unavailable'); assert.equal(sent, 0);
  const unsupported = new GuestTicketClient(slug, storage(), null, async () => { sent++; return waiting; });
  await assert.rejects(unsupported.join(), (error) => error.code === 'browser_unsupported'); assert.equal(sent, 0);
});

test('guest HTTP keeps the credential out of URL and body and never redirects to login', async () => {
  const saved = storage(); const calls = [];
  mock.method(globalThis, 'fetch', async (url, options) => {
    calls.push(url); assert.equal(url.includes(secretA), false);
    assert.equal(options.headers['X-Queue-Ticket'], secretA);
    assert.equal(options.body, '{}'); assert.equal(options.credentials, 'same-origin');
    return Response.json(waiting);
  });
  const client = new GuestTicketClient(slug, saved, locks(), undefined, () => secretA);
  await client.join(); assert.deepEqual(calls, [`/api/public/queues/${slug}/ticket`]);
});

test('public configuration uses versioned PUT with CSRF; no visitor credential is accepted', async () => {
  globalThis.document = { cookie: 'turnesta_csrf=csrf' };
  mock.method(globalThis, 'fetch', async (url, options) => {
    assert.equal(url, '/api/organizations/tenant/queues/queue/public-access');
    assert.equal(options.method, 'PUT'); assert.equal(options.headers.get('X-CSRF-Token'), 'csrf');
    assert.deepEqual(JSON.parse(options.body), { join_open: false, expected_version: 3 });
    return Response.json({ public_url: '/q/slug', qr_url: '/qr.svg', join_open: false, version: 4 });
  });
  assert.equal((await api.configureQueuePublicAccess('tenant', 'queue', false, 3)).version, 4);
  assert.equal(validTicket({ ...waiting, status: 'called', people_ahead: 0 }), false);
  assert.equal(validTicket({ ...waiting, status: 'called', people_ahead: null }), true);
});
