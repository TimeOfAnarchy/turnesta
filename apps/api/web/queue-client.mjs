export class GuestError extends Error {
  constructor(status, code) { super(code); this.status = status; this.code = code; }
}

export async function guestRequest(url, options = {}) {
  const response = await fetch(url, { ...options, credentials: 'same-origin', signal: AbortSignal.timeout(10_000) });
  const body = await response.json();
  if (!response.ok) throw new GuestError(response.status, typeof body?.code === 'string' ? body.code : 'request_failed');
  return body;
}

export function validTicket(value) {
  return value && Number.isSafeInteger(value.number) && value.number > 0
    && typeof value.service_date === 'string' && /^\d{4}-\d{2}-\d{2}$/.test(value.service_date)
    && typeof value.issued_at === 'string' && Number.isFinite(Date.parse(value.issued_at))
    && ['waiting', 'called', 'completed', 'cancelled'].includes(value.status)
    && (value.status === 'waiting' ? Number.isSafeInteger(value.people_ahead) && value.people_ahead >= 0 : value.people_ahead === null);
}

export function guestTipLink(ticket) {
  const tip = ticket?.tips;
  if (ticket?.status !== 'completed' || tip?.environment !== 'mock'
      || typeof tip.display_name !== 'string' || !tip.display_name.trim()
      || typeof tip.public_slug !== 'string' || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(tip.public_slug)) return null;
  return { href: `/tips/${encodeURIComponent(tip.public_slug)}`, name: tip.display_name };
}

function randomToken() {
  return Array.from(crypto.getRandomValues(new Uint8Array(32)), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

export class GuestTicketClient {
  constructor(slug, storage, locks, send, generate = randomToken) {
    this.slug = slug;
    this.storage = storage;
    this.locks = locks;
    this.generate = generate;
    this.storageKey = `turnesta:guest-ticket:v1:${slug}`;
    this.ticket = null;
    this.expired = false;
    this.busy = false;
    this.token = this.readToken();
    this.send = send ?? ((method, token) => guestRequest(`/api/public/queues/${encodeURIComponent(slug)}/ticket`, {
      method, headers: { 'X-Queue-Ticket': token, ...(method === 'POST' ? { 'Content-Type': 'application/json' } : {}) },
      ...(method === 'POST' ? { body: '{}' } : {}),
    }));
  }

  readToken() {
    if (!this.storage) throw new GuestError(0, 'storage_unavailable');
    let token;
    try { token = this.storage.getItem(this.storageKey); }
    catch { throw new GuestError(0, 'storage_unavailable'); }
    if (token !== null && !/^[0-9a-f]{64}$/.test(token)) throw new GuestError(0, 'saved_ticket_invalid');
    return token;
  }

  canStartNew() { return this.expired || ['completed', 'cancelled'].includes(this.ticket?.status); }

  async refresh() {
    const token = this.readToken();
    if (token !== this.token) { this.ticket = null; this.expired = false; this.token = token; }
    if (!token) return null;
    try {
      const value = await this.send('GET', token);
      if (this.token !== token || this.readToken() !== token) return this.ticket;
      if (!validTicket(value)) throw new Error('Invalid public ticket response');
      this.ticket = value; this.expired = false;
      return value;
    } catch (error) {
      if (this.token !== token || this.readToken() !== token) return this.ticket;
      if (error instanceof GuestError && error.status === 410) { this.expired = true; this.ticket = null; }
      throw error;
    }
  }

  async join(startNew = false) {
    if (this.busy) throw new GuestError(0, 'busy');
    if (startNew && !this.canStartNew()) throw new GuestError(0, 'active_ticket');
    if (!this.locks?.request) throw new GuestError(0, 'browser_unsupported');
    const previousToken = this.token;
    this.busy = true;
    try {
      // The lock coordinates cooperating tabs; the DB still enforces replay
      // safety for this token and applies quotas independently of the browser.
      return await this.locks.request(this.storageKey, async () => {
        let token = this.readToken();
        if (!token || (startNew && token === previousToken)) {
          token = this.generate();
          if (!/^[0-9a-f]{64}$/.test(token)) throw new Error('Invalid generated credential');
          try { this.storage.setItem(this.storageKey, token); }
          catch { throw new GuestError(0, 'storage_unavailable'); }
        }
        if (token !== this.token) { this.ticket = null; this.expired = false; }
        this.token = token;
        const value = await this.send('POST', token);
        if (!validTicket(value)) throw new Error('Invalid public ticket response');
        this.ticket = value; this.expired = false;
        return value;
      });
    } finally { this.busy = false; }
  }
}

export function guestErrorMessage(error) {
  if (error?.code === 'queue_closed') return 'Выдача новых талонов приостановлена. Ранее полученный талон можно проверить кнопкой «Обновить».';
  if (error?.code === 'queue_rate_limit') return 'Сейчас поступает много запросов. Подождите минуту и повторите получение.';
  if (error?.code === 'queue_capacity') return 'Очередь заполнена. Обратитесь к сотруднику или попробуйте позже.';
  if (error?.code === 'ticket_expired') return 'Срок просмотра завершённого талона истёк. Можно получить новый талон.';
  if (error?.code === 'browser_unsupported') return 'Получение талона в этом браузере недоступно. Откройте страницу в актуальном браузере или обратитесь к сотруднику.';
  if (error?.code === 'saved_ticket_invalid' || error?.code === 'storage_unavailable') return 'Не удалось прочитать сохранённый талон. Обратитесь к сотруднику точки.';
  if (error?.status === 404) return 'Талон пока не найден. Если получение было прервано, повторите его прежней кнопкой — второй талон не создастся.';
  return 'Не удалось подтвердить результат. Проверьте подключение и повторите прежний запрос или обновите состояние.';
}
