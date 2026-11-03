import assert from 'node:assert/strict';
import { test } from 'node:test';
import { typedTime } from '../src/time24.ts';

test('typed digits become a complete 24-hour time', () => {
  assert.deepEqual(typedTime('0'), { text: '0', value: null });
  assert.deepEqual(typedTime('09'), { text: '09', value: null });
  assert.deepEqual(typedTime('093'), { text: '09:3', value: null });
  assert.deepEqual(typedTime('09:30'), { text: '09:30', value: '09:30' });
  assert.deepEqual(typedTime('1745'), { text: '17:45', value: '17:45' });
  assert.deepEqual(typedTime('23:59'), { text: '23:59', value: '23:59' });
});

test('a single-digit hour before a separator is padded', () => {
  assert.deepEqual(typedTime('9:30'), { text: '09:30', value: '09:30' });
  assert.deepEqual(typedTime('9.15'), { text: '09:15', value: '09:15' });
});

test('impossible or extra input never reports a time', () => {
  assert.equal(typedTime('24:00').value, null);
  assert.equal(typedTime('12:60').value, null);
  assert.deepEqual(typedTime('12:345'), { text: '12:34', value: '12:34' });
  assert.deepEqual(typedTime('ab'), { text: '', value: null });
  assert.deepEqual(typedTime(''), { text: '', value: null });
});
