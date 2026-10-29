import assert from 'node:assert/strict';
import { test } from 'node:test';
import { watchQueue } from '../src/queueLive.mjs';
import { DisplayClient } from '../../api/web/queue-display-client.mjs';

const revision = n => String(n).padStart(64, '0');
const flush = () => new Promise(resolve => setImmediate(resolve));
function environment() {
  const sources = [], intervals = new Map(), timeouts = new Map(); let id = 0, now = 0;
  class Source {
    constructor(url) { this.url = url; this.listeners = new Map(); sources.push(this); }
    addEventListener(name, callback) { this.listeners.set(name, callback); }
    close() { this.closed = true; }
    emit(type, value = revision(1)) { this.listeners.get(type)?.({ type, lastEventId: value }); }
  }
  const page = new EventTarget(); page.hidden = false;
  const window = new EventTarget();
  const env = { document: page, window, navigator: { onLine: true }, EventSource: Source, performance: { now: () => now },
    setInterval: (callback, delay) => { intervals.set(++id, { callback, delay }); return id; }, clearInterval: key => intervals.delete(key),
    setTimeout: callback => { timeouts.set(++id, callback); return id; }, clearTimeout: key => timeouts.delete(key) };
  return { env, sources, intervals, timeouts,
    tick: delay => { for (const timer of [...intervals.values()]) if (timer.delay === delay) timer.callback(); },
    now: value => { now = value; },
    retry: () => { const entry = timeouts.entries().next().value; timeouts.delete(entry[0]); entry[1](); },
  };
}

test('healthy heartbeats confirm applied data without repeating snapshot GET', async () => {
  const e = environment(); let reads = 0, confirmed = 0; const states = [];
  const live = watchQueue({ url: '/events', refresh: async () => { reads++; return true; }, confirm: () => confirmed++, status: s => states.push(s) }, e.env);
  await flush(); e.sources[0].emit('refresh'); await flush();
  const count = reads; e.sources[0].emit('heartbeat'); e.tick(5_000); await flush();
  assert.equal(reads, count); assert.equal(confirmed, 1); assert.equal(states.at(-1), 'live');
  live.dispose(); assert.equal(e.intervals.size, 0); assert.equal(e.sources[0].closed, true);
});

test('invalidations during a slow read coalesce into a read of the latest revision', async () => {
  const e = environment(); const finishes = []; let confirmed = 0;
  const live = watchQueue({ url: '/events', refresh: () => new Promise(resolve => finishes.push(resolve)), confirm: () => confirmed++ }, e.env);
  e.sources[0].emit('refresh', revision(1)); e.sources[0].emit('refresh', revision(2)); e.sources[0].emit('heartbeat', revision(2));
  assert.equal(finishes.length, 1); assert.equal(confirmed, 0);
  finishes[0](true); await flush(); assert.equal(finishes.length, 2);
  finishes[1](true); await flush(); e.sources[0].emit('heartbeat', revision(2)); assert.equal(confirmed, 1);
  live.dispose();
});

test('a failed snapshot is retried on heartbeat and cannot renew display freshness', async () => {
  const e = environment(); let succeeds = false, confirmed = 0, reads = 0;
  const live = watchQueue({ url: '/events', refresh: async () => { reads++; return succeeds; }, confirm: () => confirmed++ }, e.env);
  await flush(); e.sources[0].emit('refresh'); await flush(); e.sources[0].emit('heartbeat'); await flush();
  assert.equal(confirmed, 0); const failedReads = reads;
  succeeds = true; e.sources[0].emit('heartbeat'); await flush(); e.sources[0].emit('heartbeat');
  assert.equal(reads, failedReads + 1); assert.equal(confirmed, 1); live.dispose();
});

test('reconnect closes the old source and always reloads even with an identical event id', async () => {
  const e = environment(); let reads = 0, disconnects = 0;
  const live = watchQueue({ url: '/events', refresh: async () => { reads++; return true; }, disconnect: () => disconnects++ }, e.env);
  await flush(); const old = e.sources[0]; old.emit('refresh'); await flush(); old.onerror(); await flush();
  assert.equal(old.closed, true); assert.equal(disconnects, 1); e.retry(); const current = e.sources[1];
  const count = reads; old.emit('refresh', revision(9)); await flush(); assert.equal(reads, count);
  current.emit('refresh'); await flush(); assert.equal(reads, count + 1); live.dispose();
});

test('offline, hidden and browser page cache transitions stop streams and resume fresh reads', async () => {
  const e = environment(); let reads = 0;
  const live = watchQueue({ url: '/events', refresh: async () => { reads++; return true; } }, e.env);
  await flush(); e.env.navigator.onLine = false; e.env.window.dispatchEvent(new Event('offline')); const count = reads;
  e.tick(5_000); await flush(); assert.equal(reads, count); assert.ok(e.sources[0].closed);
  e.env.navigator.onLine = true; e.env.window.dispatchEvent(new Event('online')); await flush(); assert.equal(e.sources.length, 2);
  e.env.document.hidden = true; e.env.document.dispatchEvent(new Event('visibilitychange')); assert.ok(e.sources[1].closed);
  e.env.document.hidden = false; e.env.document.dispatchEvent(new Event('visibilitychange')); await flush();
  e.env.window.dispatchEvent(new Event('pagehide')); assert.ok(e.sources[2].closed);
  e.env.window.dispatchEvent(new Event('pageshow')); await flush(); assert.equal(e.sources.length, 4); live.dispose();
});

test('stalled streams, unavailability and denied publication fail closed and retain fallback reads', async () => {
  const e = environment(); let reads = 0, disconnects = 0;
  const live = watchQueue({ url: '/events', refresh: async () => { reads++; return true; }, disconnect: () => disconnects++ }, e.env);
  await flush(); e.sources[0].emit('refresh'); await flush(); e.now(12_000); e.tick(1_000); await flush();
  assert.equal(disconnects, 1); const count = reads; e.tick(5_000); await flush(); assert.equal(reads, count + 1);
  e.retry(); e.sources[1].emit('denied'); assert.equal(e.sources[1].closed, true); e.retry();
  e.sources[2].emit('unavailable'); assert.equal(e.sources[2].closed, true); live.dispose();
});

test('missing EventSource falls back; disposal prevents late reads and status changes', async () => {
  const e = environment(); e.env.EventSource = undefined; let reads = 0;
  const live = watchQueue({ url: '/events', refresh: async () => { reads++; return true; } }, e.env);
  await flush(); e.tick(5_000); await flush(); assert.equal(reads, 2); live.dispose();
  e.env.window.dispatchEvent(new Event('online')); e.tick(30_000); await flush(); assert.equal(reads, 2);
});

test('heartbeat freshness requires an applied snapshot and ends at transport failure', async () => {
  const e = environment(); let now = 0;
  const value = { queue_name: 'Queue', location_name: 'Point', organization_name: 'Org', time_zone: 'UTC', version: '1', observed_at: '2026-10-28T12:00:00Z', join_open: true, waiting_count: 0, called_count: 0, page: 0, next_page: null, called: [] };
  const client = new DisplayClient('slug', async () => value, () => now);
  const live = watchQueue({ url: '/events', refresh: async () => { await client.refresh(); return Boolean(client.view()); }, confirm: () => client.confirmFreshness(), disconnect: () => client.invalidate('stale') }, e.env);
  await flush(); e.sources[0].emit('refresh'); await flush(); now = 14_000; e.sources[0].emit('heartbeat'); now = 20_000;
  assert.ok(client.view()); e.sources[0].emit('unavailable'); assert.equal(client.view(), null); live.dispose();
});

test('a buffered heartbeat after a long execution gap is rejected before it can renew data', async () => {
  const e=environment(); let confirmed=0;
  const live=watchQueue({url:'/events',refresh:async()=>true,confirm:()=>confirmed++},e.env);
  await flush(); e.sources[0].emit('refresh'); await flush(); e.now(12_001); e.sources[0].emit('heartbeat');
  assert.equal(confirmed,0); assert.ok(e.sources[0].closed); live.dispose();
});

test('an evicted snapshot is fetched again instead of being confirmed by heartbeat', async () => {
  const e=environment(); let reads=0;
  const live=watchQueue({url:'/events',refresh:async()=>{reads++;return true;},confirm:()=>false},e.env);
  await flush(); e.sources[0].emit('refresh'); await flush(); const count=reads;
  e.sources[0].emit('heartbeat'); await flush(); assert.equal(reads,count+1); live.dispose();
});
