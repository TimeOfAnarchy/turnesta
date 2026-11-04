import assert from 'node:assert/strict';
import { afterEach, mock, test } from 'node:test';
import {
  PublicBookingClient,
  BookingGuestError,
  bookingRequest,
  monthGrid,
  shiftMonth,
  timeLabel,
} from '../../api/web/bookings-client.mjs';
import { publicBookingApi, newFormField } from '../src/publicBookings.ts';
afterEach(() => {
  mock.restoreAll();
  delete globalThis.document;
});
const id = 'b69920ef-d721-461c-a3f1-8962a2faef08';
const body = () => ({
  offer_id: id,
  offer_version: 1,
  specialist_version: 1,
  form_version: 1,
  time_zone: 'Europe/Moscow',
  answers: {
    client_name: 'Private Client',
    phone: '+7 (900) 000-00-00',
    field_extra: 'Private answer',
  },
});
const receipt = {
  reference: id,
  status: 'confirmed',
  title: 'Visit',
  specialist_name: 'Doctor',
  organization_name: 'Business',
  location_name: 'Location',
  time_zone: 'Europe/Moscow',
  version: 1,
  date: '2026-12-01',
  starts_local: '2026-12-01T10:00',
  ends_local: null,
};
const token = '1'.repeat(64);
function storage() {
  const map = new Map();
  return {
    map,
    getItem: (k) => map.get(k) ?? null,
    setItem: (k, v) => map.set(k, v),
  };
}
function locks() {
  let tail = Promise.resolve();
  return {
    request: (name, options, fn) => {
      const next = tail.then(fn);
      tail = next.catch(() => {});
      return next;
    },
  };
}

test('public booking survives reload without automatic mutation and removes answers after confirmation', async () => {
  const saved = storage(),
    gate = locks();
  let original;
  const first = new PublicBookingClient(
    id,
    saved,
    gate,
    async (url, options) => {
      original = options;
      const persisted = JSON.parse(saved.getItem(first.key));
      assert.equal(
        persisted.pending.token,
        options.headers['X-Booking-Access'],
      );
      assert.deepEqual(persisted.pending.body, JSON.parse(options.body));
      throw new TypeError('reply lost');
    },
    () => token,
  );
  const input = body();
  await assert.rejects(first.submit(input));
  input.answers.client_name = 'Changed after sending';
  let calls = 0;
  const restored = new PublicBookingClient(
    id,
    saved,
    gate,
    async (url, options) => {
      calls++;
      assert.equal(options.body, original.body);
      assert.equal(options.headers['X-Booking-Access'], token);
      return receipt;
    },
    () => {
      throw new Error('must not generate a new token');
    },
  );
  assert.equal(calls, 0);
  await restored.recover();
  assert.equal(calls, 1);
  assert.equal(restored.read().pending, null);
  assert.equal(restored.read().history[0].token, token);
  assert.ok(!saved.getItem(first.key).includes('Private'));
});
test('a changed intent cannot replace an unresolved booking capability', async () => {
  const saved = storage();
  let calls = 0;
  const c = new PublicBookingClient(
    id,
    saved,
    locks(),
    async () => {
      calls++;
      throw new TypeError('offline');
    },
    () => token,
  );
  await assert.rejects(c.submit(body()));
  const different = body();
  different.answers.client_name = 'Other';
  await assert.rejects(
    c.submit(different),
    (e) => e.code === 'booking_pending',
  );
  assert.equal(calls, 1);
});
test('cross-tab locking detects a completed booking before submitting another request', async () => {
  const saved = storage(),
    gate = locks();
  let calls = 0;
  const send = async () => {
    calls++;
    return receipt;
  };
  const a = new PublicBookingClient(id, saved, gate, send, () => token),
    b = new PublicBookingClient(id, saved, gate, send, () => '2'.repeat(64));
  const results = await Promise.allSettled([
    a.submit(body()),
    b.submit(body()),
  ]);
  assert.equal(results.filter((r) => r.status === 'fulfilled').length, 1);
  assert.equal(calls, 1);
  assert.equal(
    results.find((r) => r.status === 'rejected').reason.code,
    'another_tab_booked',
  );
});
test('known transaction rejections release intent but admission errors and unknown results retain it', async () => {
  for (const code of [
    'booking_changed',
    'offer_full',
    'specialist_busy',
    'offer_unavailable',
    'booking_closed',
    'invalid_booking_form',
    'booking_rate_limit',
  ]) {
    const c = new PublicBookingClient(
      id,
      storage(),
      locks(),
      async () => {
        throw new BookingGuestError(409, code);
      },
      () => token,
    );
    await assert.rejects(c.submit(body()));
    assert.equal(c.read().pending, null);
  }
  for (const [status, code] of [
    [503, 'service_unavailable'],
    [429, 'too_many_requests'],
    [409, 'booking_request_conflict'],
    [401, 'booking_access_required'],
  ]) {
    const c = new PublicBookingClient(
      id,
      storage(),
      locks(),
      async () => {
        throw new BookingGuestError(status, code);
      },
      () => token,
    );
    await assert.rejects(c.submit(body()));
    assert.ok(c.read().pending);
  }
});
test('storage failures stop sending and preserve an already committed intent for replay', async () => {
  let sends = 0;
  const c = new PublicBookingClient(
    id,
    {
      getItem: () => null,
      setItem: () => {
        throw new Error('quota');
      },
    },
    locks(),
    async () => {
      sends++;
      return receipt;
    },
    () => token,
  );
  await assert.rejects(c.submit(body()));
  assert.equal(sends, 0);
  const saved = storage();
  const set = saved.setItem;
  let writes = 0;
  saved.setItem = (k, v) => {
    if (++writes === 2) throw new Error('cleanup failed');
    set(k, v);
  };
  const first = new PublicBookingClient(
    id,
    saved,
    locks(),
    async () => receipt,
    () => token,
  );
  await assert.rejects(first.submit(body()));
  assert.ok(first.read().pending);
  const retry = new PublicBookingClient(
    id,
    saved,
    locks(),
    async (url, options) => {
      assert.equal(options.headers['X-Booking-Access'], token);
      return receipt;
    },
  );
  await retry.recover();
  assert.equal(retry.read().pending, null);
});
test('receipt history is bounded and unavailable locks never issue an unjournaled booking', async () => {
  let generated = 0;
  const c = new PublicBookingClient(
    id,
    storage(),
    locks(),
    async () => receipt,
    () => String(++generated).padStart(64, '0'),
  );
  for (let n = 0; n < 12; n++) await c.submit(body());
  assert.equal(c.read().history.length, 10);
  const bad = new PublicBookingClient(id, storage(), null, async () => {
    throw new Error('must not send');
  });
  await assert.rejects(
    bad.submit(body()),
    (e) => e.code === 'locks_unavailable',
  );
});
test('calendar arithmetic and clock labels retain month boundaries and DST fold offsets', () => {
  assert.equal(shiftMonth('2026-12', 1), '2027-01');
  assert.equal(shiftMonth('2026-01', -1), '2025-12');
  const leap = monthGrid('2028-02').filter(Boolean);
  assert.equal(leap.length, 29);
  assert.equal(leap.at(-1), '2028-02-29');
  assert.deepEqual(monthGrid('2026-11').slice(0, 6), Array(6).fill(null));
  const label = timeLabel({
    starts_local: '2026-11-01T01:30',
    ends_local: '2026-11-01T01:10',
    starts_at: '2026-11-01T05:30:00Z',
    ends_at: '2026-11-01T06:10:00Z',
    time_zone: 'America/New_York',
  });
  assert.ok(label.includes('GMT-4') && label.includes('GMT-5'));
});
test('public transport omits staff cookies and combines caller cancellation with a timeout', async () => {
  let sent;
  mock.method(globalThis, 'fetch', async (url, options) => {
    sent = options;
    return new Response(JSON.stringify(receipt), { status: 200 });
  });
  const controller = new AbortController();
  await bookingRequest('/public', { signal: controller.signal });
  assert.equal(sent.credentials, 'omit');
  assert.ok(sent.signal instanceof AbortSignal);
  controller.abort();
  assert.equal(sent.signal.aborted, true);
});
test('form editor keeps stable field keys and sends tenant-scoped optimistic configuration', async () => {
  const field = newFormField('select');
  assert.match(field.key, /^field_[a-f0-9]{32}$/);
  assert.equal(newFormField('phone').key, 'phone');
  globalThis.document = { cookie: 'turnesta_csrf=csrf' };
  let sent;
  mock.method(globalThis, 'fetch', async (url, options) => {
    sent = { url, options };
    return new Response(JSON.stringify({ version: 3 }), { status: 200 });
  });
  await publicBookingApi.configure(
    'tenant',
    'point',
    true,
    'https://clinic.example/privacy',
    { fields: [field] },
    2,
  );
  assert.ok(sent.url.endsWith('/locations/point/bookings/public-access'));
  assert.equal(sent.options.headers.get('X-CSRF-Token'), 'csrf');
  assert.equal(JSON.parse(sent.options.body).expected_version, 2);
  assert.equal(
    JSON.parse(sent.options.body).privacy_url,
    'https://clinic.example/privacy',
  );
});
