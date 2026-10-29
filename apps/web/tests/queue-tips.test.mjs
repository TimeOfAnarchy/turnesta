import assert from 'node:assert/strict';
import { test } from 'node:test';
import { guestTipLink } from '../../api/web/queue-client.mjs';

const tip = { public_slug: 'a31cda14-e815-49ec-9538-d27a59153278', display_name: 'Команда <Лесной>', environment: 'mock' };
test('optional tip offer is available only for completed visits and constructs a relative capability-free URL', () => {
  assert.deepEqual(guestTipLink({ status: 'completed', tips: tip }), { href: `/tips/${tip.public_slug}`, name: tip.display_name });
  for (const status of ['waiting', 'called', 'cancelled']) assert.equal(guestTipLink({ status, tips: tip }), null);
  assert.equal(guestTipLink({ status: 'completed', tips: null }), null);
  assert.equal(guestTipLink(null), null);
});
test('invalid tip destination and a different environment never become a link', () => {
  for (const public_slug of ['javascript:alert(1)', '//example.test', `${tip.public_slug}?token=secret`, '', undefined]) {
    assert.equal(guestTipLink({ status: 'completed', tips: { ...tip, public_slug } }), null);
  }
  assert.equal(guestTipLink({ status: 'completed', tips: { ...tip, environment: 'production' } }), null);
  assert.equal(guestTipLink({ status: 'completed', tips: { ...tip, display_name: ' ' } }), null);
});
