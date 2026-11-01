export class BookingGuestError extends Error {
  constructor(status, code) {
    super(code);
    this.status = status;
    this.code = code;
  }
}
export async function bookingRequest(url, options = {}) {
  const signal = options.signal
    ? AbortSignal.any([options.signal, AbortSignal.timeout(10_000)])
    : AbortSignal.timeout(10_000);
  const response = await fetch(url, {
    ...options,
    credentials: 'omit',
    signal,
  });
  const body = await response.json().catch(() => null);
  if (!response.ok)
    throw new BookingGuestError(
      response.status,
      typeof body?.code === 'string' ? body.code : 'request_failed',
    );
  return body;
}
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const secret = /^[a-f0-9]{64}$/;
const stamp = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/;
function token() {
  return [...crypto.getRandomValues(new Uint8Array(32))]
    .map((v) => v.toString(16).padStart(2, '0'))
    .join('');
}
function canonical(value) {
  if (value === null || typeof value !== 'object') return JSON.stringify(value);
  return `{${Object.keys(value)
    .sort()
    .map((k) => `${JSON.stringify(k)}:${canonical(value[k])}`)
    .join(',')}}`;
}
export function validPublicInput(v) {
  return (
    v &&
    typeof v === 'object' &&
    Object.keys(v).sort().join(',') ===
      'answers,form_version,offer_id,offer_version,specialist_version,time_zone' &&
    uuid.test(v.offer_id) &&
    ['offer_version', 'specialist_version', 'form_version'].every(
      (k) => Number.isInteger(v[k]) && v[k] > 0,
    ) &&
    typeof v.time_zone === 'string' &&
    v.time_zone.length <= 128 &&
    v.answers &&
    typeof v.answers === 'object' &&
    !Array.isArray(v.answers) &&
    Object.keys(v.answers).length <= 14 &&
    Object.entries(v.answers).every(
      ([k, s]) =>
        /^[a-z][a-z0-9_]{0,39}$/.test(k) &&
        typeof s === 'string' &&
        s.length <= 2000,
    )
  );
}
export function validPublicReceipt(v) {
  return (
    v &&
    uuid.test(v.reference) &&
    ['confirmed', 'cancelled'].includes(v.status) &&
    Number.isInteger(v.version) &&
    v.version > 0 &&
    [
      'title',
      'specialist_name',
      'organization_name',
      'location_name',
      'time_zone',
    ].every((k) => typeof v[k] === 'string') &&
    (v.date === null || /^\d{4}-\d{2}-\d{2}$/.test(v.date)) &&
    (v.starts_local === null || stamp.test(v.starts_local)) &&
    (v.ends_local === null || stamp.test(v.ends_local))
  );
}
const definite = new Set([
  'booking_changed',
  'offer_full',
  'specialist_busy',
  'offer_unavailable',
  'booking_closed',
  'invalid_booking_form',
  'invalid_request',
  'booking_rate_limit',
]);
export class PublicBookingClient {
  constructor(slug, storage, locks, send = bookingRequest, generate = token) {
    this.slug = slug;
    this.storage = storage;
    this.locks = locks;
    this.send = send;
    this.generate = generate;
    this.key = `turnesta:public-booking:v1:${slug}`;
    this.busy = false;
    this.observed = this.read().history[0]?.token ?? null;
  }
  read() {
    if (!this.storage) throw new BookingGuestError(0, 'storage_unavailable');
    const raw = this.storage.getItem(this.key);
    if (raw === null) return { pending: null, history: [] };
    if (raw.length > 40_000)
      throw new BookingGuestError(0, 'storage_unavailable');
    const s = JSON.parse(raw);
    if (
      !s ||
      !Array.isArray(s.history) ||
      s.history.length > 10 ||
      !s.history.every(
        (h) => secret.test(h?.token) && typeof h.created_at === 'string',
      ) ||
      (s.pending !== null &&
        (!secret.test(s.pending?.token) ||
          !validPublicInput(s.pending?.body) ||
          typeof s.pending.created_at !== 'string'))
    )
      throw new BookingGuestError(0, 'storage_unavailable');
    return s;
  }
  write(value) {
    this.storage.setItem(this.key, JSON.stringify(value));
  }
  acknowledge() {
    this.observed = this.read().history[0]?.token ?? null;
  }
  async submit(body) {
    return this.run(body);
  }
  async recover() {
    return this.run();
  }
  async run(body) {
    if (this.busy) throw new BookingGuestError(0, 'booking_pending');
    if (!this.locks) throw new BookingGuestError(0, 'locks_unavailable');
    this.busy = true;
    try {
      return await this.locks.request(
        this.key,
        { mode: 'exclusive' },
        async () => {
          const s = this.read();
          if (!s.pending && (s.history[0]?.token ?? null) !== this.observed)
            throw new BookingGuestError(0, 'another_tab_booked');
          if (
            s.pending &&
            body &&
            canonical(body) !== canonical(s.pending.body)
          )
            throw new BookingGuestError(0, 'booking_pending');
          if (!s.pending) {
            if (!body || !validPublicInput(body))
              throw new BookingGuestError(0, 'invalid_booking_form');
            s.pending = {
              token: this.generate(),
              body: structuredClone(body),
              created_at: new Date().toISOString(),
            };
            this.write(s);
          }
          const pending = s.pending;
          let receipt;
          try {
            receipt = await this.send(
              `/api/public/bookings/${encodeURIComponent(this.slug)}/receipt`,
              {
                method: 'POST',
                headers: {
                  'Content-Type': 'application/json',
                  'X-Booking-Access': pending.token,
                },
                body: JSON.stringify(pending.body),
              },
            );
            if (!validPublicReceipt(receipt))
              throw new Error('Invalid booking receipt');
          } catch (error) {
            if (
              error instanceof BookingGuestError &&
              error.status >= 400 &&
              error.status < 500 &&
              definite.has(error.code)
            ) {
              s.pending = null;
              this.write(s);
            }
            throw error;
          }
          s.history = [
            { token: pending.token, created_at: pending.created_at },
            ...s.history.filter((h) => h.token !== pending.token),
          ].slice(0, 10);
          s.pending = null;
          this.write(s);
          this.observed = pending.token;
          return receipt;
        },
      );
    } finally {
      this.busy = false;
    }
  }
  async receipt(token) {
    if (!secret.test(token))
      throw new BookingGuestError(0, 'booking_access_required');
    const receipt = await this.send(
      `/api/public/bookings/${encodeURIComponent(this.slug)}/receipt`,
      { headers: { 'X-Booking-Access': token } },
    );
    if (!validPublicReceipt(receipt))
      throw new Error('Invalid booking receipt');
    return receipt;
  }
}
export function shiftMonth(month, delta) {
  const [year, m] = month.split('-').map(Number);
  const d = new Date(Date.UTC(year, m - 1 + delta, 1));
  return `${d.getUTCFullYear()}-${String(d.getUTCMonth() + 1).padStart(2, '0')}`;
}
export function monthGrid(month) {
  const start = new Date(`${month}-01T12:00:00Z`);
  const offset = (start.getUTCDay() + 6) % 7;
  const days = new Date(
    Date.UTC(start.getUTCFullYear(), start.getUTCMonth() + 1, 0),
  ).getUTCDate();
  return Array.from({ length: Math.ceil((offset + days) / 7) * 7 }, (_, i) =>
    i < offset || i >= offset + days
      ? null
      : `${month}-${String(i - offset + 1).padStart(2, '0')}`,
  );
}
export function dateLabel(date) {
  return date
    ? new Date(`${date}T12:00:00Z`).toLocaleDateString('ru-RU', {
        timeZone: 'UTC',
        day: 'numeric',
        month: 'long',
        year: 'numeric',
      })
    : 'Дата по согласованию';
}
export function offsetLabel(instant, zone) {
  if (!instant) return '';
  return (
    new Intl.DateTimeFormat('en-US', {
      timeZone: zone,
      timeZoneName: 'shortOffset',
    })
      .formatToParts(new Date(instant))
      .find((p) => p.type === 'timeZoneName')?.value ?? ''
  );
}
export function timeLabel(item) {
  if (!item.starts_local) return 'Время согласуем';
  let first = item.starts_local.slice(11, 16);
  if (!item.ends_local) return `${first} · без указанного окончания`;
  const startOffset = offsetLabel(item.starts_at, item.time_zone),
    endOffset = offsetLabel(item.ends_at, item.time_zone),
    changed = startOffset && endOffset && startOffset !== endOffset;
  if (changed) first += ` ${startOffset}`;
  return `${first} — ${item.ends_local.slice(0, 10) === item.starts_local.slice(0, 10) ? '' : `${dateLabel(item.ends_local.slice(0, 10))}, `}${item.ends_local.slice(11, 16)}${changed ? ` ${endOffset}` : ''}`;
}
export function guestError(error) {
  return (
    {
      booking_closed:
        'Сейчас онлайн-запись приостановлена. Обратитесь в организацию.',
      booking_changed:
        'Расписание или анкета изменились. Выберите талон заново и проверьте поля.',
      offer_full:
        'Это время уже заняли. Выберите другой талон — введённые данные сохранены.',
      specialist_busy:
        'Специалист уже занят в это время. Выберите другой талон.',
      offer_unavailable: 'Талон больше недоступен. Выберите другое время.',
      invalid_booking_form:
        'Проверьте обязательные поля, телефон, email и варианты ответа.',
      booking_pending: 'Сначала восстановите результат предыдущего запроса.',
      another_tab_booked:
        'В другой вкладке уже создали запись. Сначала проверьте её в разделе «Мои записи».',
      storage_unavailable:
        'Не удалось сохранить запрос в браузере. Разрешите хранилище и перезагрузите страницу.',
      locks_unavailable:
        'Для записи нужен браузер с поддержкой защищённого хранилища и Web Locks. Откройте страницу в современном браузере.',
      booking_request_conflict:
        'Этот запрос отличается от сохранённого. Проверьте прежнюю запись или обратитесь в организацию.',
      booking_rate_limit:
        'Сейчас поступает много записей. Попробуйте немного позже.',
      too_many_requests: 'Сервис занят. Повторите проверку чуть позже.',
      booking_access_expired:
        'Срок доступа к записи истёк. Обратитесь в организацию.',
      not_found: 'Запись или страница не найдена.',
    }[error?.code] ??
    'Не удалось связаться с сервером. Проверьте подключение и повторите проверку.'
  );
}
