import assert from 'node:assert/strict';
import { afterEach, mock, test } from 'node:test';
import { ApiError } from '../src/api.ts';
import {
  BookingCommands,
  bookingApi,
  conflictNotice,
  timeLabel,
  shiftDate,
} from '../src/bookings.ts';

afterEach(() => {
  mock.restoreAll();
  delete globalThis.document;
});
const id = '4e6bf8ac-6d1e-4e6d-aefb-f81b74d724cc';
const specialist = '4200b74b-3029-4e50-985d-6f0457efcc1e';
const scope = { tenant: 'tenant', location: 'point', user: 'actor' };
const command = (timing) => ({
  type: 'save_offer',
  id,
  specialist_id: specialist,
  time_zone: 'Europe/Moscow',
  title: 'Visit',
  timing,
  state: 'ready',
  expected_version: 0,
  conflict_token: null,
});
const receipt = { resource: { kind: 'offer', item: { id, version: 1 } } };
function storage() {
  const values = new Map();
  return {
    values,
    getItem: (k) => values.get(k) ?? null,
    setItem: (k, v) => values.set(k, v),
    removeItem: (k) => values.delete(k),
  };
}

test('every timing mode survives reload with exactly the original payload and no automatic replay', async () => {
  for (const timing of [
    {
      mode: 'interval',
      starts_local: '2026-11-05T10:00',
      ends_local: '2026-11-05T10:30',
    },
    { mode: 'start_only', starts_local: '2026-11-05T10:00' },
    { mode: 'by_agreement', date: null, places: 10 },
  ]) {
    const saved = storage();
    let sent;
    const first = new BookingCommands(scope, saved, async (s, p) => {
      sent = structuredClone(p);
      assert.deepEqual(JSON.parse(saved.getItem(first.storageKey)), p);
      throw new TypeError('lost reply');
    });
    await assert.rejects(first.run(command(timing)));
    let calls = 0;
    const restored = new BookingCommands(scope, saved, async (s, p) => {
      calls++;
      assert.deepEqual(p, sent);
      return receipt;
    });
    assert.equal(calls, 0);
    await assert.rejects(
      restored.run(command({ mode: 'by_agreement', date: null, places: 3 })),
      /unresolved/,
    );
    await restored.run();
    assert.equal(calls, 1);
    assert.equal(restored.pending, null);
  }
});

test('an overlap rejection releases intent; acknowledgement is a fresh request with the same offer identity', async () => {
  const saved = storage(),
    sent = [];
  let calls = 0;
  const first = command({
    mode: 'start_only',
    starts_local: '2026-11-05T10:00',
  });
  const journal = new BookingCommands(scope, saved, async (s, p) => {
    sent.push(structuredClone(p));
    if (calls++ === 0) throw new ApiError(409, 'booking_overlap');
    return receipt;
  });
  await assert.rejects(journal.run(first));
  assert.equal(journal.pending, null);
  await journal.run({ ...first, conflict_token: 'ab'.repeat(32) });
  assert.notEqual(sent[0].key, sent[1].key);
  assert.equal(sent[0].command.id, sent[1].command.id);
  assert.equal(sent[0].command.conflict_token, null);
});

test('uncertain outcomes retain the journal; recovery with changed timezone rejects definitively', async () => {
  const saved = storage();
  const first = new BookingCommands(scope, saved, async () => {
    throw new ApiError(503, 'service_unavailable');
  });
  await assert.rejects(
    first.run(command({ mode: 'by_agreement', date: null, places: 2 })),
  );
  assert.ok(first.pending);
  const login = new BookingCommands(scope, saved, async () => {
    throw new ApiError(401, 'unauthorized');
  });
  await assert.rejects(login.run());
  assert.ok(login.pending);
  const restored = new BookingCommands(scope, saved, async () => {
    throw new ApiError(409, 'booking_time_zone_changed');
  });
  await assert.rejects(restored.run());
  assert.equal(restored.pending, null);
});

test('scope and strict stored payload validation prevent sending another location or a corrupt intention', async () => {
  const saved = storage();
  const first = new BookingCommands(scope, saved, async () => {
    throw new TypeError('lost');
  });
  await assert.rejects(
    first.run(
      command({ mode: 'start_only', starts_local: '2026-11-05T10:00' }),
    ),
  );
  const other = new BookingCommands(
    { ...scope, location: 'different' },
    saved,
    async () => receipt,
  );
  assert.equal(other.pending, null);
  const raw = JSON.parse(saved.getItem(first.storageKey));
  raw.command.timing.ends_local = '2026-11-05T11:00';
  saved.setItem(first.storageKey, JSON.stringify(raw));
  let sends = 0;
  const invalid = new BookingCommands(scope, saved, async () => {
    sends++;
    return receipt;
  });
  assert.ok(invalid.storageError);
  await assert.rejects(invalid.run());
  assert.equal(sends, 0);
});

test('a persistence failure prevents HTTP and concurrent clicks cannot send another intention', async () => {
  let sends = 0;
  const blocked = new BookingCommands(
    scope,
    {
      getItem: () => null,
      setItem: () => {
        throw new Error('full');
      },
      removeItem: () => {},
    },
    async () => {
      sends++;
      return receipt;
    },
  );
  await assert.rejects(
    blocked.run(command({ mode: 'by_agreement', date: null, places: 1 })),
  );
  assert.equal(sends, 0);
  let release;
  const journal = new BookingCommands(
    scope,
    storage(),
    () =>
      new Promise((resolve) => {
        release = resolve;
      }),
  );
  const work = journal.run(
    command({ mode: 'by_agreement', date: null, places: 1 }),
  );
  await assert.rejects(journal.run(), /unresolved/);
  release(receipt);
  await work;
});

test('booking command sends CSRF, original key, explicit timezone and a bounded request signal', async () => {
  globalThis.document = { cookie: 'turnesta_csrf=csrf-value' };
  const calls = [];
  mock.method(globalThis, 'fetch', async (url, init) => {
    calls.push({ url, init });
    return new Response(JSON.stringify(receipt), { status: 200 });
  });
  const pending = {
    key: id,
    command: command({ mode: 'start_only', starts_local: '2026-11-05T10:00' }),
    created_at: '2026-11-01T12:00:00Z',
  };
  await bookingApi.command(scope, pending);
  const { url, init } = calls[0];
  assert.ok(url.includes('/locations/point/bookings/commands'));
  assert.equal(init.headers.get('Idempotency-Key'), id);
  assert.equal(init.headers.get('X-CSRF-Token'), 'csrf-value');
  assert.equal(init.credentials, 'same-origin');
  assert.ok(init.signal instanceof AbortSignal);
  assert.deepEqual(JSON.parse(init.body), pending.command);
});

test('malformed success keeps the exact intention available for recovery', async () => {
  globalThis.document = { cookie: '' };
  mock.method(
    globalThis,
    'fetch',
    async () =>
      new Response(
        JSON.stringify({
          resource: { kind: 'offer', item: { id: 'wrong', version: 1 } },
        }),
        { status: 200 },
      ),
  );
  const journal = new BookingCommands(scope, storage());
  await assert.rejects(
    journal.run(command({ mode: 'by_agreement', date: null, places: 10 })),
  );
  assert.ok(journal.pending);
});

test('time suggestions use the point timezone and preserve the caller cancellation signal', async () => {
  globalThis.document = { cookie: '' };
  let seen;
  mock.method(globalThis, 'fetch', async (url, init) => {
    seen = { url, init };
    return new Response(
      JSON.stringify({
        ends_local: '2026-03-08T04:30',
        time_zone: 'America/New_York',
      }),
      { status: 200 },
    );
  });
  const controller = new AbortController();
  const result = await bookingApi.suggestTime(
    'org',
    'point',
    '2026-03-08T01:30',
    120,
    'America/New_York',
    id,
    controller.signal,
  );
  const q = new URL(seen.url, 'https://example.test').searchParams;
  assert.equal(q.get('time_zone'), 'America/New_York');
  assert.equal(q.get('starts_local'), '2026-03-08T01:30');
  assert.equal(q.get('offer_id'), id);
  assert.equal(seen.init.signal, controller.signal);
  assert.equal(result.ends_local, '2026-03-08T04:30');
});

test('structured overlap data is validated and unknown endings remain explicit', () => {
  const details = {
    token: 'ab'.repeat(32),
    total: 1,
    conflicts: [
      {
        id,
        title: 'Visit <script>',
        starts_local: '2026-11-05T10:00',
        ends_local: null,
        time_zone: 'Europe/Moscow',
      },
    ],
  };
  assert.equal(
    conflictNotice(new ApiError(409, 'booking_overlap', details)),
    details,
  );
  assert.equal(
    conflictNotice(
      new ApiError(409, 'booking_overlap', { ...details, token: 'bad' }),
    ),
    null,
  );
  assert.equal(
    timeLabel({ starts_local: null, ends_local: null }),
    'Время согласуем',
  );
  assert.equal(timeLabel(details.conflicts[0]), '10:00 · окончание не указано');
  assert.equal(
    timeLabel({
      starts_local: '2026-11-05T23:00',
      ends_local: '2026-11-06T00:30',
    }),
    '23:00 — 06.11 00:30',
  );
  assert.equal(shiftDate('2026-12-31', 1), '2027-01-01');
});
