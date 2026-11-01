import { ApiError, request } from './api.ts';
import type { Page } from './api.ts';
import {
  isAppointmentAction,
  validAppointmentCommand,
} from './appointments.ts';
import type { Appointment, AppointmentCommand } from './appointments.ts';

export type Specialist = {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  version: number;
};
export type Preset = {
  id: string;
  name: string;
  duration_minutes: number | null;
  enabled: boolean;
  version: number;
};
export type OfferState = 'draft' | 'ready' | 'withdrawn';
export type Timing =
  | { mode: 'interval'; starts_local: string; ends_local: string }
  | { mode: 'start_only'; starts_local: string }
  | { mode: 'by_agreement'; date: string | null; places: number };
export type Offer = {
  id: string;
  specialist_id: string;
  title: string;
  mode: Timing['mode'];
  date: string | null;
  starts_local: string | null;
  ends_local: string | null;
  starts_at: string | null;
  ends_at: string | null;
  time_zone: string;
  places: number;
  state: OfferState;
  version: number;
  reserved_places: number | null;
  blocked_by_booking: boolean | null;
};
export type BookingCommand =
  | AppointmentCommand
  | {
      type: 'save_specialist';
      id: string;
      name: string;
      description: string;
      enabled: boolean;
      expected_version: number;
    }
  | {
      type: 'save_preset';
      id: string;
      name: string;
      duration_minutes: number | null;
      enabled: boolean;
      expected_version: number;
    }
  | {
      type: 'save_offer';
      id: string;
      specialist_id: string;
      time_zone: string;
      title: string;
      timing: Timing;
      state: OfferState;
      expected_version: number;
      conflict_token: string | null;
    };
export type BookingReceipt = {
  resource:
    | { kind: 'specialist'; item: Specialist }
    | { kind: 'preset'; item: Preset }
    | { kind: 'offer'; item: Offer }
    | { kind: 'appointment'; item: Appointment };
};
export type ConflictNotice = {
  token: string;
  total: number;
  conflicts: {
    id: string;
    title: string;
    starts_local: string | null;
    ends_local: string | null;
    time_zone: string;
  }[];
};
type Scope = { tenant: string; location: string; user: string };
export type BookingPending = {
  key: string;
  command: BookingCommand;
  created_at: string;
};
type JournalStorage = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const base = (tenant: string, location: string) =>
  `/api/organizations/${encodeURIComponent(tenant)}/locations/${encodeURIComponent(location)}/bookings`;
const paging = (after: string | null) =>
  new URLSearchParams({ limit: '50', ...(after ? { after } : {}) });
export const bookingApi = {
  suggestTime: async (
    tenant: string,
    location: string,
    starts_local: string,
    minutes: number,
    time_zone: string,
    offer_id: string | null,
    signal: AbortSignal,
  ) => {
    const result = await request<{ ends_local: string; time_zone: string }>(
      `${base(tenant, location)}/suggest-time?${new URLSearchParams({ starts_local, minutes: String(minutes), time_zone, ...(offer_id ? { offer_id } : {}) })}`,
      { signal },
    );
    if (
      typeof result?.ends_local !== 'string' ||
      !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(result.ends_local) ||
      result.time_zone !== time_zone
    )
      throw new Error('Invalid time suggestion');
    return result;
  },
  specialists: (
    tenant: string,
    location: string,
    after: string | null,
    signal: AbortSignal,
  ) =>
    request<Page<Specialist>>(
      `${base(tenant, location)}/specialists?${paging(after)}`,
      { signal },
    ),
  presets: (
    tenant: string,
    location: string,
    after: string | null,
    signal: AbortSignal,
  ) =>
    request<Page<Preset>>(
      `${base(tenant, location)}/presets?${paging(after)}`,
      { signal },
    ),
  offers: (
    tenant: string,
    location: string,
    specialist: string,
    date: string,
    withdrawn: boolean,
    after: string | null,
    signal: AbortSignal,
  ) =>
    request<Page<Offer>>(
      `${base(tenant, location)}/offers?${paging(after)}&${new URLSearchParams({ specialist_id: specialist, date, include_withdrawn: String(withdrawn) })}`,
      { signal },
    ),
  command: async (scope: Scope, pending: BookingPending) => {
    const result = await request<BookingReceipt>(
      `${base(scope.tenant, scope.location)}/${isAppointmentAction(pending.command) ? 'appointments/' : ''}commands`,
      {
        method: 'POST',
        signal: AbortSignal.timeout(10_000),
        headers: { 'Idempotency-Key': pending.key },
        body: JSON.stringify(pending.command),
      },
    );
    if (
      !result?.resource ||
      !['specialist', 'preset', 'offer', 'appointment'].includes(
        result.resource.kind,
      ) ||
      !uuid.test(result.resource.item?.id) ||
      !Number.isInteger(result.resource.item.version) ||
      result.resource.item.version < 1
    )
      throw new Error('Invalid booking receipt');
    return result;
  },
};
function validCommand(value: unknown): value is BookingCommand {
  if (!value || typeof value !== 'object') return false;
  const v = value as Record<string, unknown>;
  if (isAppointmentAction(v)) return validAppointmentCommand(v);
  if (
    typeof v.id !== 'string' ||
    !uuid.test(v.id) ||
    !Number.isInteger(v.expected_version) ||
    (v.expected_version as number) < 0
  )
    return false;
  const keys = Object.keys(v).sort().join(',');
  if (v.type === 'save_specialist')
    return (
      keys === 'description,enabled,expected_version,id,name,type' &&
      typeof v.name === 'string' &&
      typeof v.description === 'string' &&
      typeof v.enabled === 'boolean'
    );
  if (v.type === 'save_preset')
    return (
      keys === 'duration_minutes,enabled,expected_version,id,name,type' &&
      typeof v.name === 'string' &&
      typeof v.enabled === 'boolean' &&
      (v.duration_minutes === null ||
        (Number.isInteger(v.duration_minutes) &&
          (v.duration_minutes as number) > 0))
    );
  if (
    v.type !== 'save_offer' ||
    keys !==
      'conflict_token,expected_version,id,specialist_id,state,time_zone,timing,title,type' ||
    typeof v.specialist_id !== 'string' ||
    !uuid.test(v.specialist_id) ||
    typeof v.time_zone !== 'string' ||
    typeof v.title !== 'string' ||
    !['draft', 'ready', 'withdrawn'].includes(v.state as string) ||
    !(
      v.conflict_token === null ||
      (typeof v.conflict_token === 'string' &&
        /^[a-f0-9]{64}$/i.test(v.conflict_token))
    )
  )
    return false;
  const t = v.timing as Record<string, unknown> | null;
  if (!t || typeof t !== 'object') return false;
  const tk = Object.keys(t).sort().join(',');
  return (
    (t.mode === 'interval' &&
      tk === 'ends_local,mode,starts_local' &&
      typeof t.starts_local === 'string' &&
      typeof t.ends_local === 'string') ||
    (t.mode === 'start_only' &&
      tk === 'mode,starts_local' &&
      typeof t.starts_local === 'string') ||
    (t.mode === 'by_agreement' &&
      tk === 'date,mode,places' &&
      (t.date === null || typeof t.date === 'string') &&
      Number.isInteger(t.places) &&
      (t.places as number) > 0)
  );
}
export class BookingCommands {
  readonly storageKey: string;
  pending: BookingPending | null = null;
  storageError: unknown = null;
  busy = false;
  readonly scope: Scope;
  private storage: JournalStorage | null;
  private send: typeof bookingApi.command;
  constructor(
    scope: Scope,
    storage: JournalStorage | null,
    send = bookingApi.command,
  ) {
    this.scope = scope;
    this.storage = storage;
    this.send = send;
    this.storageKey = `turnesta:bookings-command:v1:${[scope.tenant, scope.location, scope.user].map(encodeURIComponent).join(':')}`;
    try {
      if (!storage) throw new Error('Storage unavailable');
      const raw = storage.getItem(this.storageKey);
      if (raw !== null) {
        if (raw.length > 16384) throw new Error('Invalid saved command');
        const p = JSON.parse(raw);
        if (
          typeof p?.key !== 'string' ||
          !uuid.test(p.key) ||
          !validCommand(p.command) ||
          typeof p.created_at !== 'string' ||
          !Number.isFinite(Date.parse(p.created_at))
        )
          throw new Error('Invalid saved command');
        this.pending = p;
      }
    } catch (error) {
      this.storageError = error;
    }
  }
  private clear() {
    try {
      this.storage!.removeItem(this.storageKey);
      this.pending = null;
    } catch (error) {
      this.storageError = error;
      throw error;
    }
  }
  async run(command?: BookingCommand): Promise<BookingReceipt> {
    if (this.storageError) throw this.storageError;
    if (this.busy || (command && this.pending) || (!command && !this.pending))
      throw new Error('Command unresolved');
    const recovering = Boolean(this.pending);
    this.busy = true;
    try {
      if (command) {
        const pending = {
          key: crypto.randomUUID(),
          command: structuredClone(command),
          created_at: new Date().toISOString(),
        };
        try {
          this.storage!.setItem(this.storageKey, JSON.stringify(pending));
        } catch (error) {
          this.storageError = error;
          throw error;
        }
        this.pending = pending;
      }
      let result: BookingReceipt;
      try {
        result = await this.send(this.scope, this.pending!);
      } catch (error) {
        if (
          error instanceof ApiError &&
          error.status >= 400 &&
          error.status < 500 &&
          error.status !== 401 &&
          error.code !== 'idempotency_conflict' &&
          (!recovering ||
            [
              'version_conflict',
              'booking_overlap',
              'invalid_booking',
              'time_gap',
              'time_fold',
              'specialist_disabled',
              'booking_time_zone_changed',
              'invalid_client',
              'offer_unavailable',
              'offer_full',
              'specialist_busy',
              'offer_occupied',
              'appointment_closed',
            ].includes(error.code))
        )
          this.clear();
        throw error;
      }
      this.clear();
      return result;
    } finally {
      this.busy = false;
    }
  }
}
export function conflictNotice(error: unknown): ConflictNotice | null {
  if (!(error instanceof ApiError) || error.code !== 'booking_overlap')
    return null;
  const d = error.details as ConflictNotice;
  if (
    !d ||
    typeof d.token !== 'string' ||
    !/^[a-f0-9]{64}$/i.test(d.token) ||
    !Number.isSafeInteger(d.total) ||
    d.total < 1 ||
    !Array.isArray(d.conflicts) ||
    d.conflicts.length > 20 ||
    !d.conflicts.every(
      (c) =>
        c &&
        typeof c.id === 'string' &&
        uuid.test(c.id) &&
        typeof c.title === 'string' &&
        typeof c.starts_local === 'string' &&
        (c.ends_local === null || typeof c.ends_local === 'string') &&
        typeof c.time_zone === 'string',
    )
  )
    return null;
  return d;
}
export function bookingError(error: unknown): string | undefined {
  if (!(error instanceof ApiError)) return undefined;
  return (
    {
      invalid_client: 'Проверьте имя, телефон, email и длину комментария.',
      offer_unavailable:
        'Талон закрыт для новых записей или специалист выключен. Выберите другой талон.',
      offer_full:
        'Последнее место уже заняли. Обновите расписание и выберите другой талон.',
      specialist_busy:
        'В это время у специалиста уже есть клиент. Выберите другой талон.',
      offer_occupied:
        'На талон уже записаны клиенты. Для изменения времени сначала перенесите или отмените их записи; лимит не может быть меньше числа записей.',
      appointment_closed: 'Запись уже отменена. Обновите данные.',
      offline:
        'Нет подключения. После восстановления связи сохраните изменения вручную.',
      booking_time_zone_changed:
        'Часовой пояс изменился. Проверьте время и повторите сохранение.',
      invalid_booking: 'Проверьте дату, время, длительность и количество мест.',
      time_gap:
        'Такого местного времени нет из-за перевода часов. Выберите другое время.',
      time_fold:
        'Это время встречается дважды при переводе часов. Выберите однозначное время.',
      specialist_disabled:
        'Специалист выключен. Сохраните черновик или включите специалиста.',
      booking_overlap:
        'Есть пересекающиеся предложения. Проверьте их перед сохранением.',
      version_conflict:
        'Эту запись уже изменили. Ваш черновик сохранён в форме; загрузите актуальную версию перед повтором.',
      idempotency_conflict:
        'Запрос отличается от сохранённого. Обратитесь к администратору.',
    } as Record<string, string>
  )[error.code];
}
export const offerStateLabels = {
  draft: 'Черновик',
  ready: 'В расписании',
  withdrawn: 'Снят с расписания',
};
export function todayAt(zone: string): string {
  const parts = new Intl.DateTimeFormat('en-CA', {
    timeZone: zone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).formatToParts(new Date());
  const get = (type: string) => parts.find((p) => p.type === type)!.value;
  return `${get('year')}-${get('month')}-${get('day')}`;
}
export function shiftDate(date: string, days: number): string {
  const d = new Date(`${date}T12:00:00Z`);
  d.setUTCDate(d.getUTCDate() + days);
  return d.toISOString().slice(0, 10);
}
export function timeLabel(
  offer: Pick<Offer, 'starts_local' | 'ends_local'>,
): string {
  if (!offer.starts_local) return 'Время согласуем';
  const begin = offer.starts_local.slice(11, 16);
  if (!offer.ends_local) return `${begin} · окончание не указано`;
  const end =
    offer.ends_local.slice(0, 10) === offer.starts_local.slice(0, 10)
      ? offer.ends_local.slice(11, 16)
      : `${offer.ends_local.slice(8, 10)}.${offer.ends_local.slice(5, 7)} ${offer.ends_local.slice(11, 16)}`;
  return `${begin} — ${end}`;
}
