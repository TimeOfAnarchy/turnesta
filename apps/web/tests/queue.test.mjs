import assert from 'node:assert/strict';
import { afterEach, mock, test } from 'node:test';
import { api, ApiError } from '../src/api.ts';
import { QueueCommands, canCloseTicket, ticketLabel } from '../src/queueCommands.ts';

afterEach(() => { mock.restoreAll(); delete globalThis.document; });
const scope = { tenant: 'tenant', queue: 'queue', user: 'actor' };
const ticketId = '94bb2597-94fe-40c9-ae86-856a4ebc8116';
const ticket = { id: ticketId, queue_id: scope.queue, number: 1, service_date: '2026-10-26', status: 'waiting', called_by: null };
const receipt = { queue_id: scope.queue, version: 2, ticket };

function storage() {
  const entries = new Map();
  return { entries, getItem: (key) => entries.get(key) ?? null, setItem: (key, value) => entries.set(key, value), removeItem: (key) => entries.delete(key) };
}

test('operator service choice survives journal reload unchanged and definitive unavailability clears it',async()=>{
  const saved=storage(),service_id='4e6bf8ac-6d1e-4e6d-aefb-f81b74d724cc';
  const first=new QueueCommands(scope,saved,async()=>{throw new TypeError('lost reply');});
  await assert.rejects(first.run({type:'issue',service_id}));
  const oldKey=first.pending.key;
  const restored=new QueueCommands(scope,saved,async(s,p)=>{assert.equal(p.key,oldKey);assert.deepEqual(p.command,{type:'issue',service_id});throw new ApiError(409,'service_unavailable');});
  await assert.rejects(restored.run());assert.equal(restored.pending,null);
});

test('lost response survives reload; only explicit retry sends the same key and payload', async () => {
  const saved = storage();
  const requests = [];
  const first = new QueueCommands(scope, saved, async (_scope, pending) => {
    assert.deepEqual(JSON.parse(saved.getItem(first.storageKey)), pending, 'journal must exist before HTTP');
    requests.push(structuredClone(pending));
    throw new TypeError('response lost after server commit');
  });
  await assert.rejects(first.run({ type: 'issue' }));
  await assert.rejects(first.run({ type: 'call_next' }), /unresolved/);
  const restored = new QueueCommands(scope, saved, async (actualScope, pending) => {
    assert.deepEqual(actualScope, scope);
    requests.push(structuredClone(pending));
    return receipt;
  });
  assert.equal(requests.length, 1, 'loading the journal must not send anything');
  assert.deepEqual(restored.pending, requests[0]);
  assert.deepEqual(await restored.run(), receipt);
  assert.deepEqual(requests[0], requests[1]);
  assert.equal(restored.pending, null);
  assert.equal(saved.getItem(first.storageKey), null);
});

test('a fresh intention gets a fresh key even when CallNext was empty', async () => {
  const keys = [];
  const journal = new QueueCommands(scope, storage(), async (_scope, pending) => {
    keys.push(pending.key);
    return { queue_id: scope.queue, version: 1, ticket: null };
  });
  await journal.run({ type: 'call_next' });
  await journal.run({ type: 'call_next' });
  assert.notEqual(keys[0], keys[1]);
});

test('uncertain errors and expired login retain the command; definitive rejection releases it', async () => {
  for (const error of [new ApiError(503, 'service_unavailable'), new TypeError('network'), new SyntaxError('json'), new ApiError(401, 'unauthorized'), new ApiError(409, 'idempotency_conflict')]) {
    const journal = new QueueCommands(scope, storage(), async () => { throw error; });
    await assert.rejects(journal.run({ type: 'issue' }));
    assert.ok(journal.pending);
  }
  for (const status of [400, 403, 404, 409, 429]) {
    const journal = new QueueCommands(scope, storage(), async () => { throw new ApiError(status, 'state_conflict'); });
    await assert.rejects(journal.run({ type: 'issue' }));
    assert.equal(journal.pending, null);
  }
});

test('concurrent clicks cannot send another command while one is in flight', async () => {
  let finish;
  let calls = 0;
  const journal = new QueueCommands(scope, storage(), () => { calls++; return new Promise((resolve) => { finish = resolve; }); });
  const sending = journal.run({ type: 'issue' });
  await assert.rejects(journal.run({ type: 'call_next' }));
  await assert.rejects(journal.run());
  assert.equal(calls, 1);
  finish(receipt);
  await sending;
  assert.equal(journal.busy, false);
});

test('rejected recovery does not erase a possibly committed original command', async () => {
  const saved = storage();
  const initial = new QueueCommands(scope, saved, async () => { throw new TypeError('lost response'); });
  await assert.rejects(initial.run({ type: 'issue' }));
  const original = structuredClone(initial.pending);
  // An authorization/validation failure occurs before receipt lookup and
  // cannot establish whether the original request committed.
  for (const status of [400, 403, 404, 429]) {
    const restored = new QueueCommands(scope, saved, async () => { throw new ApiError(status, 'forbidden'); });
    await assert.rejects(restored.run());
    assert.deepEqual(restored.pending, original);
  }
  // These domain conflicts are returned after the serialized receipt lookup:
  // a committed original command would instead have returned its receipt.
  const rejected = new QueueCommands(scope, saved, async () => { throw new ApiError(409, 'state_conflict'); });
  await assert.rejects(rejected.run());
  assert.equal(rejected.pending, null);
});

test('pending journals are isolated by actor, tenant and queue; corrupt data blocks sending', async () => {
  const saved = storage();
  const journal = new QueueCommands(scope, saved, async () => { throw new TypeError('network'); });
  await assert.rejects(journal.run({ type: 'issue' }));
  for (const other of [{ ...scope, user: 'other' }, { ...scope, tenant: 'other' }, { ...scope, queue: 'other' }]) {
    assert.equal(new QueueCommands(other, saved).pending, null);
  }
  saved.setItem(journal.storageKey, '{broken-json');
  let sent = false;
  const corrupted = new QueueCommands(scope, saved, async () => { sent = true; return receipt; });
  await assert.rejects(corrupted.run({ type: 'issue' }));
  assert.ok(corrupted.storageError);
  assert.equal(sent, false);
});

test('failed persistence prevents sending; failed cleanup blocks subsequent new commands', async () => {
  let calls = 0;
  const send = async () => { calls++; return receipt; };
  const unavailable = new QueueCommands(scope, { ...storage(), setItem: () => { throw new Error('quota'); } }, send);
  await assert.rejects(unavailable.run({ type: 'issue' }));
  assert.equal(calls, 0);
  const unremovable = new QueueCommands(scope, { ...storage(), removeItem: () => { throw new Error('storage unavailable'); } }, send);
  await assert.rejects(unremovable.run({ type: 'issue' }));
  await assert.rejects(unremovable.run({ type: 'issue' }));
  assert.equal(calls, 1);
  assert.ok(unremovable.pending);
});

test('Queue API binds numeric cursor, current-user filter, CSRF and supplied command key', async () => {
  globalThis.document = { cookie: 'turnesta_csrf=csrf' };
  const controller = new AbortController();
  mock.method(globalThis, 'fetch', async (url, options) => {
    if (options.method === 'POST') {
      assert.equal(url, '/api/organizations/tenant/queues/queue/commands');
      assert.equal(options.headers.get('Idempotency-Key'), 'original-key');
      assert.equal(options.headers.get('X-CSRF-Token'), 'csrf');
      assert.equal(options.credentials, 'same-origin');
      assert.ok(options.signal instanceof AbortSignal);
      assert.deepEqual(JSON.parse(options.body), { type: 'complete', ticket_id: ticketId });
      return Response.json({ ...receipt, ticket: { ...ticket, status: 'completed' } });
    }
    assert.equal(url, '/api/organizations/tenant/queues/queue/tickets?status=called&limit=50&after=51&mine=true');
    assert.equal(options.signal, controller.signal);
    return Response.json({ items: [], next_cursor: 102 });
  });
  assert.equal((await api.tickets('tenant', 'queue', 'called', 51, controller.signal, true)).next_cursor, 102);
  await api.queueCommand('tenant', 'queue', 'original-key', { type: 'complete', ticket_id: ticketId });
});

test('malformed 200 response preserves the journal and never retries automatically', async () => {
  globalThis.document = { cookie: 'turnesta_csrf=csrf' };
  let calls = 0;
  mock.method(globalThis, 'fetch', async () => { calls++; return Response.json({ queue_id: scope.queue, version: 2, ticket: null }); });
  const journal = new QueueCommands(scope, storage());
  await assert.rejects(journal.run({ type: 'issue' }), /Invalid queue command response/);
  assert.equal(calls, 1);
  assert.ok(journal.pending);
});

test('ticket labels distinguish days; operators close their own calls and managers can override', () => {
  assert.equal(ticketLabel(ticket), '№ 001 · 26.10.2026');
  assert.notEqual(ticketLabel(ticket), ticketLabel({ ...ticket, service_date: '2026-10-25' }));
  assert.equal(canCloseTicket(ticket, 'employee', scope.user), false);
  assert.equal(canCloseTicket(ticket, 'operator', scope.user), true);
  const called = { ...ticket, status: 'called', called_by: 'other' };
  assert.equal(canCloseTicket(called, 'operator', scope.user), false);
  assert.equal(canCloseTicket(called, 'manager', scope.user), true);
  assert.equal(canCloseTicket({ ...called, called_by: scope.user }, 'operator', scope.user), true);
  assert.equal(canCloseTicket({ ...called, status: 'completed' }, 'owner', scope.user), false);
});
