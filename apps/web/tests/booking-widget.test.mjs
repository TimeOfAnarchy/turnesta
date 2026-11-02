import assert from 'node:assert/strict';
import { test, mock, afterEach } from 'node:test';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import {
  validWidgetMessage,
  probeBookingStorage,
} from '../../api/web/booking-widget-protocol.mjs';
import { bookingWidgetApi, widgetSnippet } from '../src/bookingWidget.ts';

afterEach(() => {
  mock.restoreAll();
  delete globalThis.document;
});
const script = readFileSync(
  new URL('../../api/web/booking-widget.js', import.meta.url),
  'utf8',
);
const slug = 'b69920ef-d721-461c-a3f1-8962a2faef08',
  service = 'https://turnesta.example';
const booking = `${service}/book/${slug}`;
function host(count = 1, origin = 'https://clinic.example') {
  const listeners = new Set(),
    timers = new Map(),
    observers = [];
  let nextTimer = 0;
  function element(tag) {
    return {
      tag,
      dataset: {},
      style: {},
      children: [],
      attrs: {},
      events: {},
      isConnected: true,
      contentWindow:
        tag === 'iframe'
          ? {
              sent: [],
              postMessage(data, origin) {
                this.sent.push({ data, origin });
              },
            }
          : null,
      append(...nodes) {
        this.children.push(...nodes);
      },
      replaceChildren(...nodes) {
        this.children = nodes;
      },
      insertBefore(node, before) {
        this.children.splice(this.children.indexOf(before), 0, node);
      },
      setAttribute(k, v) {
        this.attrs[k] = v;
      },
      addEventListener(k, fn) {
        this.events[k] = fn;
      },
      removeEventListener(k) {
        delete this.events[k];
      },
    };
  }
  const containers = Array.from({ length: count }, () => {
    const c = element('div');
    c.dataset.turnestaBooking = booking;
    return c;
  });
  const window = {
    addEventListener(type, fn) {
      if (type === 'message') listeners.add(fn);
    },
    removeEventListener(type, fn) {
      if (type === 'message') listeners.delete(fn);
    },
  };
  window.parent = window;
  const document = {
    currentScript: { src: `${service}/assets/booking-widget.js` },
    readyState: 'complete',
    documentElement: element('html'),
    querySelectorAll() {
      return containers;
    },
    createElement: element,
  };
  const context = vm.createContext({
    window,
    document,
    location: { origin },
    URL,
    crypto,
    setTimeout(fn) {
      timers.set(++nextTimer, fn);
      return nextTimer;
    },
    clearTimeout(id) {
      timers.delete(id);
    },
    MutationObserver: class {
      constructor(fn) {
        this.fn = fn;
        observers.push(this);
      }
      observe() {}
      disconnect() {
        this.closed = true;
      }
    },
  });
  vm.runInContext(script, context);
  return {
    containers,
    listeners,
    timers,
    observers,
    context,
    frames: () =>
      containers.flatMap((c) => c.children.filter((n) => n.tag === 'iframe')),
    emit(frame, data, from = service, source = frame.contentWindow) {
      for (const fn of [...listeners]) fn({ data, origin: from, source });
    },
  };
}
const channelOf = (frame) => new URL(frame.src).searchParams.get('channel');
const resize = (frame, height, extra = {}) => ({
  v: 1,
  type: 'turnesta:resize',
  channel: channelOf(frame),
  height,
  ...extra,
});

test('host validates origin, frame, channel, type and exact fields before resizing', () => {
  const h = host(2),
    [a, b] = h.frames();
  assert.notEqual(channelOf(a), channelOf(b));
  for (const [data, origin, source] of [
    [resize(a, 900), 'https://evil.example', a.contentWindow],
    [resize(a, 900), service, b.contentWindow],
    [resize(b, 900), service, a.contentWindow],
    [resize(a, 900, { answers: 'private' }), service, a.contentWindow],
    [resize(a, Infinity), service, a.contentWindow],
    [resize(a, -1), service, a.contentWindow],
    [resize(a, 100_001), service, a.contentWindow],
    [resize(a, 900, { v: 2 }), service, a.contentWindow],
  ]) {
    h.emit(a, data, origin, source);
    assert.equal(a.style.height, '720px');
  }
  h.emit(a, resize(a, 980));
  assert.equal(a.style.height, '980px');
  assert.equal(b.style.height, '720px');
  h.emit(a, resize(a, 10_000));
  assert.equal(a.style.height, '2400px');
  h.emit(a, resize(a, 450));
  assert.equal(a.style.height, '450px');
  h.emit(a, resize(a, 100));
  assert.equal(a.style.height, '320px');
  assert.equal(a.referrerPolicy, 'no-referrer');
  assert.equal(a.attrs.sandbox, 'allow-scripts allow-same-origin allow-forms');
});
test('handshake restarts after frame load and duplicate script does not mount twice', () => {
  const h = host(),
    [frame] = h.frames();
  h.emit(frame, { v: 1, type: 'turnesta:ready', channel: channelOf(frame) });
  assert.ok(
    frame.contentWindow.sent.every(
      (m) => m.origin === service && m.data.type === 'turnesta:init',
    ),
  );
  h.emit(frame, resize(frame, 600));
  assert.equal(h.timers.size, 0);
  frame.events.load();
  assert.equal(h.timers.size, 1);
  h.emit(frame, resize(frame, 610));
  assert.equal(h.timers.size, 0);
  vm.runInContext(script, h.context);
  assert.equal(h.frames().length, 1);
  assert.equal(h.listeners.size, 1);
  frame.isConnected = false;
  h.observers[0].fn();
  assert.equal(h.listeners.size, 0);
  assert.ok(h.observers[0].closed);
});
test('host keeps a plain link when embed cannot load and refuses same-origin sandbox', () => {
  const h = host();
  const callback = [...h.timers.values()][0];
  callback();
  assert.ok(
    h.containers[0].children.some(
      (n) => n.tag === 'p' && n.textContent?.includes('Не удалось'),
    ),
  );
  assert.equal(h.containers[0].children.at(-1).children[0].href, booking);
  const same = host(1, service);
  assert.equal(same.frames().length, 0);
  assert.equal(same.containers[0].children.at(-1).children[0].href, booking);
});
test('child protocol rejects ambiguous and extra messages', () => {
  const channel = crypto.randomUUID(),
    data = { v: 1, type: 'turnesta:init', channel };
  assert.ok(validWidgetMessage(data, channel, 'turnesta:init'));
  for (const bad of [
    null,
    [],
    { ...data, reference: 'x' },
    { ...data, v: 2 },
    { ...data, type: 'turnesta:ready' },
    { ...data, channel: 'wrong' },
  ])
    assert.equal(validWidgetMessage(bad, channel, 'turnesta:init'), false);
  assert.ok(
    validWidgetMessage(
      { v: 1, type: 'turnesta:resize', channel, height: 600 },
      channel,
      'turnesta:resize',
    ),
  );
  assert.equal(
    validWidgetMessage(
      { v: 1, type: 'turnesta:resize', channel, height: NaN },
      channel,
      'turnesta:resize',
    ),
    false,
  );
});
test('storage probe checks a real write under a bounded lock and preserves existing journal', async () => {
  const data = new Map([['journal', 'private-pending']]);
  const storage = {
    setItem: (k, v) => data.set(k, v),
    getItem: (k) => data.get(k),
    removeItem: (k) => data.delete(k),
  };
  let options;
  const locks = {
    request: async (k, o, fn) => {
      options = o;
      return fn();
    },
  };
  await probeBookingStorage(storage, locks);
  assert.equal(data.size, 1);
  assert.equal(data.get('journal'), 'private-pending');
  assert.ok(options.signal instanceof AbortSignal);
  await assert.rejects(probeBookingStorage(storage, null));
  await assert.rejects(
    probeBookingStorage(
      {
        ...storage,
        setItem() {
          throw new Error('denied');
        },
      },
      locks,
    ),
  );
  await assert.rejects(
    probeBookingStorage(
      {
        ...storage,
        getItem() {
          return null;
        },
      },
      locks,
    ),
  );
  assert.equal(data.get('journal'), 'private-pending');
});
test('settings API and copied snippet carry no credentials or client state', async () => {
  globalThis.document = { cookie: 'turnesta_csrf=csrf' };
  let sent;
  mock.method(globalThis, 'fetch', async (url, options) => {
    sent = { url, options };
    return new Response(JSON.stringify({ version: 1 }), { status: 200 });
  });
  await bookingWidgetApi.configure(
    'tenant',
    'point',
    ['https://clinic.example'],
    0,
  );
  assert.ok(sent.url.endsWith('/bookings/widget'));
  assert.deepEqual(JSON.parse(sent.options.body), {
    allowed_origins: ['https://clinic.example'],
    expected_version: 0,
  });
  assert.equal(sent.options.headers.get('X-CSRF-Token'), 'csrf');
  const snippet = widgetSnippet(booking, `${service}/assets/booking-widget.js`);
  assert.ok(snippet.includes('referrerpolicy="no-referrer"'));
  assert.ok(snippet.includes('rel="noopener noreferrer"'));
  assert.ok(!/token|csrf|capability|answers/.test(snippet));
  assert.ok(
    widgetSnippet('https://example/"<>&', 'https://example/"').includes(
      '&quot;&lt;&gt;&amp;',
    ),
  );
});
