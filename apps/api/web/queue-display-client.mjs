export class DisplayError extends Error {
  constructor(code) { super(code); this.code = code; }
}

export function validDisplay(value) {
  if (!value || !['queue_name', 'location_name', 'organization_name', 'time_zone'].every(key => typeof value[key] === 'string')) return false;
  if (typeof value.version !== 'string' || !/^[1-9][0-9]*$/.test(value.version)
    || typeof value.observed_at !== 'string' || !Number.isFinite(Date.parse(value.observed_at)) || typeof value.join_open !== 'boolean') return false;
  if (![value.waiting_count, value.called_count, value.page].every(n => Number.isSafeInteger(n) && n >= 0) || value.page > 21_474_835) return false;
  const remaining = Math.max(value.called_count - value.page * 100, 0);
  if (value.next_page !== (remaining > 100 ? value.page + 1 : null)
    || !Array.isArray(value.called) || value.called.length !== Math.min(100, remaining)) return false;
  return value.called.every(ticket => Number.isSafeInteger(ticket?.number) && ticket.number > 0
    && typeof ticket.service_date === 'string' && /^\d{4}-\d{2}-\d{2}$/.test(ticket.service_date));
}

export async function loadDisplay(send, signal) {
  signal?.throwIfAborted();
  const first = await send(0, signal);
  if (!validDisplay(first) || first.page !== 0) throw new DisplayError('invalid_response');
  const called = [...first.called];
  let page = first.next_page;
  while (page !== null) {
    signal?.throwIfAborted();
    const next = await send(page, signal);
    if (!validDisplay(next) || next.page !== page) throw new DisplayError('invalid_response');
    if (next.version !== first.version || next.called_count !== first.called_count || next.waiting_count !== first.waiting_count) throw new DisplayError('snapshot_changed');
    called.push(...next.called); page = next.next_page;
  }
  signal?.throwIfAborted();
  if (new Set(called.map(ticket => `${ticket.service_date}:${ticket.number}`)).size !== called.length) throw new DisplayError('invalid_response');
  return { ...first, called, next_page: null };
}

export class DisplayClient {
  constructor(slug, send, now = () => performance.now()) {
    this.now = now;
    this.send = send ?? (async (page, signal) => {
      const response = await fetch(`/api/public/queues/${encodeURIComponent(slug)}/display?page=${page}`, { signal, credentials: 'omit', cache: 'no-store' });
      if (!response.ok) throw new DisplayError(response.status === 404 ? 'unpublished' : 'unavailable');
      return response.json();
    });
    this.snapshot = null; this.receivedAt = 0; this.busy = false; this.error = null; this.generation = 0;
  }
  view() {
    if (this.snapshot && this.now() - this.receivedAt >= 15_000) { this.snapshot = null; this.error = new DisplayError('stale'); }
    return this.snapshot;
  }
  confirmFreshness() {
    if (!this.snapshot || this.busy) return false;
    this.receivedAt = this.now(); return true;
  }
  invalidate(code) {
    this.generation++; this.controller?.abort(); this.busy = false; this.snapshot = null; this.error = new DisplayError(code);
  }
  async refresh() {
    if (this.busy) return;
    const generation = ++this.generation;
    const controller = new AbortController(); this.controller = controller; this.busy = true;
    const timeout = setTimeout(() => controller.abort(), 8_000);
    try {
      const snapshot = await loadDisplay(this.send, controller.signal);
      if (generation !== this.generation) return;
      this.snapshot = snapshot; this.receivedAt = this.now(); this.error = null;
    } catch (error) {
      if (generation !== this.generation) return;
      this.snapshot = null; this.error = error;
    } finally {
      clearTimeout(timeout);
      if (generation === this.generation) this.busy = false;
    }
  }
}

export function displayErrorMessage(error) {
  if (error?.code === 'unpublished') return 'Табло отключено или ещё не опубликовано. Обратитесь к сотруднику точки.';
  if (error?.code === 'offline') return 'Нет подключения к сети. Актуальные вызовы недоступны — обратитесь к сотруднику.';
  if (error?.code === 'snapshot_changed') return 'Очередь изменилась во время обновления. Получаем новые данные…';
  return 'Нет актуальных данных. Проверяем связь с сервером; уточните свой вызов у сотрудника.';
}
