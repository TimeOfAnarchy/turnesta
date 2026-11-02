import assert from 'node:assert/strict';
import { test, mock, afterEach } from 'node:test';
import { ApiError } from '../src/api.ts';
import { BookingCommands, bookingApi, bookingError } from '../src/bookings.ts';
import {
  batchPreview,
  validBatchCommand,
  validBatchReceipt,
} from '../src/bookingBatch.ts';
afterEach(() => {
  mock.restoreAll();
  delete globalThis.document;
});
const id = 'b69920ef-d721-461c-a3f1-8962a2faef08';
const scope = { tenant: 'tenant', location: 'point', user: 'actor' };
const request = () => ({
  id,
  specialist_id: id,
  time_zone: 'Europe/Moscow',
  state: 'ready',
  source: {
    kind: 'copy_day',
    source_date: '2030-05-01',
    target_dates: ['2030-05-20', '2030-05-21'],
  },
});
const command = () => ({
  type: 'save_batch',
  request: request(),
  preview_token: 'a'.repeat(64),
  allow_alternatives: false,
});
const receipt = {
  resource: {
    kind: 'batch',
    item: {
      id,
      version: 1,
      specialist_id: id,
      dates: ['2030-05-20'],
      offer_ids: [id],
    },
  },
};
function storage() {
  const map = new Map();
  return {
    getItem: (k) => map.get(k) ?? null,
    setItem: (k, v) => map.set(k, v),
    removeItem: (k) => map.delete(k),
  };
}

test('batch intents and bounded receipts reject extra actor fields and forged shapes', () => {
  assert.ok(validBatchCommand(command()));
  const series = command();
  series.request.source = {
    kind: 'preset_series',
    preset_id: id,
    dates: ['2030-05-20'],
    start_time: '09:00',
    end_time: '12:00',
    gap_minutes: 15,
  };
  assert.ok(validBatchCommand(series));
  for (const bad of [
    { ...command(), created_by: id },
    { ...command(), preview_token: null },
    { ...command(), allow_alternatives: 1 },
    { ...command(), request: { ...request(), state: 'withdrawn' } },
    {
      ...command(),
      request: { ...request(), source: { ...request().source, clients: [] } },
    },
  ])
    assert.equal(validBatchCommand(bad), false);
  assert.ok(validBatchReceipt(receipt.resource.item));
  for (const item of [
    { ...receipt.resource.item, offer_ids: [] },
    { ...receipt.resource.item, offer_ids: [id, id] },
    { ...receipt.resource.item, dates: [] },
    { ...receipt.resource.item, offer_ids: Array(201).fill(id) },
  ])
    assert.equal(validBatchReceipt(item), false);
});
test('copy and series recovery send exactly the original batch token consent and key', async () => {
  for (const source of [
    request().source,
    {
      kind: 'preset_series',
      preset_id: id,
      dates: ['2030-05-20'],
      start_time: '09:00',
      end_time: '12:00',
      gap_minutes: 0,
    },
  ]) {
    const s = storage();
    let original;
    const input = command();
    input.request.source = source;
    input.allow_alternatives = true;
    const first = new BookingCommands(scope, s, async (_, pending) => {
      original = structuredClone(pending);
      assert.deepEqual(JSON.parse(s.getItem(first.storageKey)), pending);
      throw new TypeError('lost reply');
    });
    await assert.rejects(first.run(input));
    let count = 0;
    const restored = new BookingCommands(scope, s, async (_, pending) => {
      count++;
      assert.deepEqual(pending, original);
      return receipt;
    });
    assert.equal(count, 0);
    await restored.run();
    assert.equal(count, 1);
    assert.equal(restored.pending, null);
  }
});
test('definitive batch rejection releases pending; unknown result preserves it', async () => {
  for (const code of [
    'invalid_batch',
    'batch_preview_changed',
    'batch_duplicate',
    'batch_overlap',
    'batch_time',
  ]) {
    const s = storage();
    const first = new BookingCommands(scope, s, async () => {
      throw new Error('offline');
    });
    await assert.rejects(first.run(command()));
    const restored = new BookingCommands(scope, s, async () => {
      throw new ApiError(409, code);
    });
    await assert.rejects(restored.run());
    assert.equal(restored.pending, null);
  }
  const s = storage();
  const first = new BookingCommands(scope, s, async () => {
    throw new ApiError(503, 'service_unavailable');
  });
  await assert.rejects(first.run(command()));
  assert.ok(first.pending);
});
test('batch preview is a scoped CSRF request and successful command uses common journal endpoint', async () => {
  globalThis.document = { cookie: 'turnesta_csrf=csrf' };
  const calls = [];
  const preview = {
    token: 'a'.repeat(64),
    rows: [
      {
        index: 1,
        title: 'Visit',
        mode: 'by_agreement',
        date: '2030-05-20',
        starts_local: null,
        ends_local: null,
        starts_at: null,
        ends_at: null,
        time_zone: 'Europe/Moscow',
        places: 2,
        state: 'ready',
      },
    ],
    conflicts: [],
    has_duplicates: false,
    conflicts_truncated: false,
  };
  mock.method(globalThis, 'fetch', async (url, options) => {
    calls.push({ url, options });
    return new Response(
      JSON.stringify(url.endsWith('/batch-preview') ? preview : receipt),
      { status: 200 },
    );
  });
  const controller = new AbortController();
  await batchPreview(
    scope.tenant,
    scope.location,
    request(),
    controller.signal,
  );
  assert.ok(calls[0].url.endsWith('/bookings/batch-preview'));
  assert.equal(calls[0].options.headers.get('X-CSRF-Token'), 'csrf');
  assert.deepEqual(JSON.parse(calls[0].options.body), request());
  controller.abort();
  assert.equal(calls[0].options.signal.aborted, true);
  await bookingApi.command(scope, {
    key: id,
    command: command(),
    created_at: new Date().toISOString(),
  });
  assert.ok(calls[1].url.endsWith('/bookings/commands'));
  assert.equal(calls[1].options.headers.get('Idempotency-Key'), id);
});
test('malformed batch success cannot clear the saved request', async () => {
  globalThis.document = { cookie: 'turnesta_csrf=csrf' };
  mock.method(
    globalThis,
    'fetch',
    async () =>
      new Response(
        JSON.stringify({
          resource: { kind: 'batch', item: { id, version: 1 } },
        }),
        { status: 200 },
      ),
  );
  const journal = new BookingCommands(scope, storage());
  await assert.rejects(journal.run(command()));
  assert.ok(journal.pending);
});
test('batch errors explain stale previews and identify DST date without echoing arbitrary input', () => {
  assert.match(
    bookingError(new ApiError(409, 'batch_preview_changed')),
    /предпросмотр/,
  );
  assert.match(
    bookingError(
      new ApiError(400, 'batch_time', { date: '2026-03-08', code: 'time_gap' }),
    ),
    /2026-03-08/,
  );
  assert.equal(
    bookingError(
      new ApiError(400, 'batch_time', { date: '<script>', code: 'time_fold' }),
    ),
    undefined,
  );
});
