import assert from 'node:assert/strict';
import { test } from 'node:test';
import { startSection, sameServices } from '../src/navigation.ts';

test('entry ignores hidden or invalid last section and always has a reachable fallback', () => {
  assert.equal(startSection('queues', ['bookings'], 'owner'), 'bookings');
  assert.equal(startSection('tips', ['queues'], 'manager'), 'locations');
  assert.equal(startSection('queues', [], 'employee'), 'locations');
  assert.equal(startSection(null, ['bookings'], 'owner'), 'bookings');
  assert.equal(startSection('../tips', ['tips'], 'owner'), 'locations');
  assert.equal(startSection('team', ['bookings'], 'operator'), 'bookings');
  assert.equal(startSection('team', [], 'owner'), 'team');
  assert.equal(startSection('settings', [], 'employee'), 'settings');
  assert.equal(startSection('queues', ['queues'], 'employee'), 'queues');
});

test('service draft detects set changes while ignoring presentation order', () => {
  assert.equal(sameServices(['queues', 'bookings'], ['bookings', 'queues']), true);
  assert.equal(sameServices(['bookings'], ['queues']), false);
  assert.equal(sameServices([], ['tips']), false);
  assert.equal(sameServices([], []), true);
});
