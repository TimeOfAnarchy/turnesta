export const liveLabels = {
  connecting: 'Подключаем обновления', live: 'Обновляется автоматически',
  reconnecting: 'Восстанавливаем связь', paused: 'Обновление приостановлено',
};

export function watchQueue(options, environment = globalThis) {
  return new QueueLive(options, environment);
}

class QueueLive {
  constructor(options, env) {
    this.options = options; this.env = env; this.page = env.document; this.window = env.window ?? env;
    this.generation = 0; this.disposed = false; this.refreshing = false; this.dirty = false;
    this.revision = null; this.applied = null; this.source = null; this.retry = 1_000; this.lastSignal = 0;
    this.state = 'paused'; this.started = false; this.now = () => env.performance.now();
    this.activity = () => {
      if (!this.active()) { this.disconnect('paused'); return; }
      this.disconnect('connecting'); this.open(); this.refresh();
    };
    this.hide = () => { this.suspended = true; this.disconnect('paused'); };
    this.show = () => { if (this.suspended) { this.suspended = false; this.activity(); } };
    this.window.addEventListener('online', this.activity); this.window.addEventListener('offline', this.activity);
    this.window.addEventListener('pagehide', this.hide); this.window.addEventListener('pageshow', this.show);
    this.page.addEventListener('visibilitychange', this.activity);
    this.fallback = env.setInterval(() => { if (this.active() && this.state !== 'live') this.refresh(); }, 5_000);
    this.safety = env.setInterval(() => { if (this.active()) this.refresh(); }, 30_000);
    this.deadline = env.setInterval(() => {
      if (this.active() && this.source && this.now() - this.lastSignal >= 12_000) this.failed();
    }, 1_000);
    this.activity(); this.started = true;
  }
  active() { return !this.disposed && !this.suspended && !this.page.hidden && this.env.navigator.onLine; }
  status(state) {
    if (!this.disposed && this.state !== state) { this.state = state; this.options.status?.(state); }
  }
  disconnect(state) {
    this.generation++; this.source?.close(); this.source = null; this.applied = null; this.revision = null;
    this.env.clearTimeout(this.retryTimer); if (this.started) this.options.disconnect?.(); this.status(state);
  }
  open() {
    if (!this.active() || this.source) return;
    if (!this.env.EventSource) { this.status('reconnecting'); return; }
    const source = new this.env.EventSource(this.options.url); this.source = source; this.lastSignal = this.now();
    const message = event => {
      if (this.source !== source || !this.active()) return;
      if (this.now() - this.lastSignal >= 12_000) { this.failed(); return; }
      if (!/^[0-9a-f]{64}$/.test(event.lastEventId)) { this.failed(); return; }
      this.lastSignal = this.now(); this.retry = 1_000; this.revision = event.lastEventId;
      if (event.type === 'refresh' || this.applied !== this.revision) this.refresh();
      else if (!this.refreshing) {
        if (this.options.confirm?.() === false) { this.applied = null; this.refresh(); }
        else this.status('live');
      }
    };
    source.addEventListener('refresh', message); source.addEventListener('heartbeat', message);
    source.addEventListener('unavailable', () => { if (this.source === source) this.failed(); });
    source.addEventListener('denied', () => { if (this.source === source) this.failed(); });
    source.onerror = () => { if (this.source === source) this.failed(); };
  }
  failed() {
    this.disconnect('reconnecting');
    if (!this.active()) return;
    this.refresh();
    const delay = this.retry * (.8 + Math.random() * .4); this.retry = Math.min(this.retry * 2, 10_000);
    this.retryTimer = this.env.setTimeout(() => { this.open(); }, delay);
  }
  refresh() {
    if (!this.active()) return;
    this.dirty = true;
    if (!this.refreshing) void this.read();
  }
  async read() {
    this.refreshing = true;
    try {
      while (this.dirty && this.active()) {
        this.dirty = false;
        const generation = this.generation; const revision = this.revision;
        let success = false;
        try { success = await this.options.refresh(); } catch { /* snapshot UI owns its error */ }
        if (!this.active()) return;
        if (generation !== this.generation) { this.dirty = true; continue; }
        this.applied = success ? revision : null;
        if (success && revision && this.source && revision === this.revision) this.status('live');
        else if (!success) this.status('reconnecting');
        if (revision !== this.revision) this.dirty = true;
      }
    } finally { this.refreshing = false; }
  }
  dispose() {
    this.disposed = true; this.generation++; this.source?.close(); this.source = null;
    this.env.clearTimeout(this.retryTimer);
    for (const timer of [this.fallback, this.safety, this.deadline]) this.env.clearInterval(timer);
    this.window.removeEventListener('online', this.activity); this.window.removeEventListener('offline', this.activity);
    this.window.removeEventListener('pagehide', this.hide); this.window.removeEventListener('pageshow', this.show);
    this.page.removeEventListener('visibilitychange', this.activity);
  }
}
