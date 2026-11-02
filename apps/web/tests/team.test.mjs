import { test, mock, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import {
  TeamCommands,
  validTeamCommand,
  validTeamReceipt,
  teamApi,
  teamError,
} from '../src/team.ts';
import { ApiError } from '../src/api.ts';
const id = '4c73ae83-2d3b-46a4-a175-045c8ac939a5';
const scope = { tenant: 'tenant', user: 'owner' };
const invite = {
  type: 'invite',
  display_name: 'Colleague',
  email: 'colleague@example.test',
  role: 'operator',
};
const receipt = {
  user_id: id,
  role: 'operator',
  active: true,
  version: 1,
  delivery: { state: 'queued', retry_after: '2030-05-01T10:00:00Z' },
};
function storage() {
  const values = new Map();
  return {
    getItem: (k) => values.get(k) ?? null,
    setItem: (k, v) => values.set(k, v),
    removeItem: (k) => values.delete(k),
  };
}
afterEach(() => {
  mock.restoreAll();
  delete globalThis.document;
});
test('team command schemas disallow owner grants actor fields and invalid versions', () => {
  for (const c of [
    invite,
    {
      type: 'set_member',
      user_id: id,
      role: 'employee',
      active: false,
      expected_version: 1,
    },
    { type: 'send_login', user_id: id, expected_version: 2 },
  ])
    assert.ok(validTeamCommand(c));
  for (const c of [
    { ...invite, role: 'owner' },
    { ...invite, actor_id: id },
    { type: 'send_login', user_id: id, expected_version: 0 },
    {
      type: 'set_member',
      user_id: id,
      role: 'manager',
      active: 1,
      expected_version: 1,
    },
  ])
    assert.equal(validTeamCommand(c), false);
  assert.ok(validTeamReceipt(receipt));
  assert.ok(
    validTeamReceipt({
      ...receipt,
      delivery: { ...receipt.delivery, state: 'deferred' },
    }),
  );
  for (const r of [
    { ...receipt, version: 0 },
    { ...receipt, delivery: {} },
    { ...receipt, active: null },
    { ...receipt, user_id: 'other' },
  ])
    assert.equal(validTeamReceipt(r), false);
});
test('lost team reply persists exact command and explicit recovery reuses the key', async () => {
  const s = storage();
  let original;
  const first = new TeamCommands(scope, s, async (_, pending) => {
    original = structuredClone(pending);
    assert.deepEqual(JSON.parse(s.getItem(first.storageKey)), pending);
    throw new TypeError('lost reply');
  });
  await assert.rejects(first.run(invite));
  assert.ok(first.pending);
  const restored = new TeamCommands(scope, s, async (actual, pending) => {
    assert.deepEqual(actual, scope);
    assert.deepEqual(pending, original);
    return receipt;
  });
  await assert.rejects(restored.run(invite));
  assert.deepEqual(await restored.run(), receipt);
  assert.equal(restored.pending, null);
  assert.equal(s.getItem(restored.storageKey), null);
});
test('definitive team errors clear pending and unknown errors retain it', async () => {
  for (const code of [
    'team_version_conflict',
    'invalid_name',
    'team_member_exists',
    'team_rate_limited',
    'team_owner_protected',
  ]) {
    const j = new TeamCommands(scope, storage(), async () => {
      throw new ApiError(409, code);
    });
    await assert.rejects(j.run(invite));
    assert.equal(j.pending, null);
    assert.ok(teamError(new ApiError(409, code)));
  }
  for (const e of [
    new ApiError(503, 'service_unavailable'),
    new ApiError(409, 'idempotency_conflict'),
    new ApiError(401, 'unauthorized'),
    new ApiError(403, 'forbidden'),
  ]) {
    const j = new TeamCommands(scope, storage(), async () => {
      throw e;
    });
    await assert.rejects(j.run(invite));
    assert.ok(j.pending);
  }
});
test('team malformed receipts and unavailable storage cannot lose or send commands', async () => {
  const j = new TeamCommands(scope, storage(), async () => ({ user_id: id }));
  await assert.rejects(j.run(invite));
  assert.ok(j.pending);
  let sent = false;
  const blocked = new TeamCommands(scope, null, async () => {
    sent = true;
    return receipt;
  });
  await assert.rejects(blocked.run(invite));
  assert.equal(sent, false);
  const invalid = storage();
  invalid.setItem(
    j.storageKey,
    JSON.stringify({ key: id, command: { ...invite, role: 'owner' } }),
  );
  assert.ok(new TeamCommands(scope, invalid).storageError);
});
test('team transport scopes the route sends CSRF and idempotency and has a timeout', async () => {
  globalThis.document = { cookie: 'turnesta_csrf=csrf' };
  const calls = [];
  mock.method(globalThis, 'fetch', async (url, options) => {
    calls.push({ url, options });
    return new Response(JSON.stringify(receipt), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  });
  await teamApi.send(scope, { key: id, command: invite });
  assert.equal(calls[0].url, '/api/organizations/tenant/team/commands');
  assert.equal(calls[0].options.headers.get('X-CSRF-Token'), 'csrf');
  assert.equal(calls[0].options.headers.get('Idempotency-Key'), id);
  assert.ok(calls[0].options.signal);
  assert.deepEqual(JSON.parse(calls[0].options.body), invite);
});
