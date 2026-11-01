import assert from 'node:assert/strict';
import { afterEach, mock, test } from 'node:test';
import { ApiError } from '../src/api.ts';
import { BookingCommands, bookingApi } from '../src/bookings.ts';
import {
  appointmentApi,
  canRecord,
  occupancyLabel,
} from '../src/appointments.ts';

afterEach(() => {
  mock.restoreAll();
  delete globalThis.document;
});
const id = '1c5a7882-04fd-4fb8-a6f0-7ddfe9195b80',
  spec = '0b046df6-0554-4cce-890b-771597fc299a',
  offer = 'c718748e-4985-49ce-bb01-a4381b9ef222';
const scope = { tenant: 'tenant', location: 'point', user: 'actor' };
const client = {
  client_name: 'Client',
  phone: '+7 (900) 000-00-00',
  email: 'client@example.test',
  note: 'Call first',
};
const command = {
  type: 'record_appointment',
  id,
  specialist_id: spec,
  offer_id: offer,
  expected_offer_version: 2,
  client,
};
const receipt = { resource: { kind: 'appointment', item: { id, version: 1 } } };
function storage() {
  const values = new Map();
  return {
    values,
    getItem: (k) => values.get(k) ?? null,
    setItem: (k, v) => values.set(k, v),
    removeItem: (k) => values.delete(k),
  };
}

test('record, contact edit, move and cancellation survive reload with the original command and key', async () => {
  for (const payload of [
    command,
    {
      type: 'update_client',
      id,
      specialist_id: spec,
      expected_version: 1,
      client,
    },
    {
      type: 'move_appointment',
      id,
      specialist_id: spec,
      offer_id: offer,
      expected_offer_version: 2,
      expected_version: 1,
    },
    {
      type: 'cancel_appointment',
      id,
      specialist_id: spec,
      expected_version: 1,
    },
  ]) {
    const saved = storage();
    let original;
    const first = new BookingCommands(scope, saved, async (s, p) => {
      original = structuredClone(p);
      throw new TypeError('reply lost');
    });
    await assert.rejects(first.run(payload));
    let calls = 0;
    const restored = new BookingCommands(scope, saved, async (s, p) => {
      calls++;
      assert.deepEqual(p, original);
      return receipt;
    });
    assert.equal(calls, 0);
    await restored.run();
    assert.equal(calls, 1);
    assert.equal(saved.getItem(restored.storageKey), null);
  }
});
test('definitive occupancy rejection releases the command while unknown outcomes keep it', async () => {
  for (const code of [
    'offer_full',
    'specialist_busy',
    'offer_unavailable',
    'offer_occupied',
    'appointment_closed',
    'invalid_client',
    'version_conflict',
  ]) {
    const saved = storage();
    const first = new BookingCommands(scope, saved, async () => {
      throw new ApiError(503, 'unavailable');
    });
    await assert.rejects(first.run(command));
    const restored = new BookingCommands(scope, saved, async () => {
      throw new ApiError(409, code);
    });
    await assert.rejects(restored.run());
    assert.equal(restored.pending, null);
  }
});
test('saved appointment commands reject extra actor, contact and status fields', () => {
  for (const bad of [
    { ...command, actor_id: id },
    { ...command, client: { ...client, status: 'confirmed' } },
    { ...command, expected_offer_version: 0 },
    { ...command, offer_id: 'bad' },
  ]) {
    const saved = storage();
    const journal = new BookingCommands(scope, saved, async () => receipt);
    saved.setItem(
      journal.storageKey,
      JSON.stringify({
        key: id,
        command: bad,
        created_at: new Date().toISOString(),
      }),
    );
    assert.ok(
      new BookingCommands(scope, saved, async () => receipt).storageError,
    );
  }
});
test('only appointment actions use the private command endpoint with CSRF and timeout', async () => {
  globalThis.document = { cookie: 'turnesta_csrf=csrf' };
  let sent;
  mock.method(globalThis, 'fetch', async (url, options) => {
    sent = { url, options };
    return new Response(JSON.stringify(receipt), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    });
  });
  await bookingApi.command(scope, {
    key: id,
    command,
    created_at: new Date().toISOString(),
  });
  assert.ok(sent.url.endsWith('/bookings/appointments/commands'));
  assert.equal(sent.options.headers.get('X-CSRF-Token'), 'csrf');
  assert.equal(sent.options.headers.get('Idempotency-Key'), id);
  assert.ok(sent.options.signal instanceof AbortSignal);
  assert.deepEqual(JSON.parse(sent.options.body), command);
});
test('private appointment lists send scope and pagination, with no contact search parameters', async () => {
  const urls = [];
  mock.method(globalThis, 'fetch', async (url) => {
    urls.push(url);
    return new Response(JSON.stringify({ items: [], next_cursor: null }), {
      status: 200,
    });
  });
  const signal = new AbortController().signal;
  await appointmentApi.list(
    'tenant',
    'point',
    spec,
    '2026-11-20',
    true,
    id,
    signal,
  );
  await appointmentApi.get('tenant', 'point', id, signal);
  assert.ok(urls[0].includes('include_cancelled=true'));
  assert.ok(urls[0].includes(`after=${id}`));
  assert.ok(!urls[0].includes('phone'));
  assert.ok(urls[1].endsWith(`/appointments/${id}`));
});
test('unknown occupancy never becomes a free slot; capacity and alternative bookings disable recording', () => {
  const o = {
    state: 'ready',
    reserved_places: 0,
    blocked_by_booking: false,
    places: 2,
  };
  assert.equal(canRecord(o, true), true);
  for (const changed of [
    { reserved_places: null },
    { reserved_places: 2 },
    { blocked_by_booking: true },
    { state: 'withdrawn' },
    { state: 'draft' },
  ])
    assert.equal(canRecord({ ...o, ...changed }, true), false);
  assert.equal(canRecord(o, false), false);
  assert.equal(
    occupancyLabel({ ...o, reserved_places: null }),
    'Занятость доступна оператору',
  );
  assert.equal(occupancyLabel({ ...o, reserved_places: 1 }), 'Свободно 1 из 2');
});
