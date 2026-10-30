export type Role = 'owner' | 'manager' | 'operator' | 'employee';
export type OrganizationSummary = { id: string; name: string; role: Role };
export type Organization = { id: string; name: string; version: number };
export type Location = { id: string; tenant_id: string; name: string; time_zone: string; version: number };
export type Page<T, Cursor = string> = { items: T[]; next_cursor: Cursor | null };
export type LocationDraft = { name: string; time_zone: string };
export type TipRecipient = { id: string; tenant_id: string; location_id: string; display_name: string; public_slug: string; environment: 'mock' };
export type TipRecipientDraft = { display_name: string; location_id: string };
export type TipAttemptStatus = 'awaiting_payment' | 'succeeded' | 'failed' | 'expired';
export type TipAttempt = { id: string; recipient_id: string; recipient_name: string; amount_minor: number; currency: 'RUB'; status: TipAttemptStatus; environment: 'mock'; created_at: string };
export type Queue = { id: string; tenant_id: string; location_id: string; name: string; version: number };
export type Service = { id: string; name: string; description: string; enabled: boolean; version: number };
export type ServiceDraft = Pick<Service, 'name' | 'description' | 'enabled'>;
export type TicketStatus = 'waiting' | 'called' | 'completed' | 'cancelled';
export type Ticket = {
  id: string; queue_id: string; position: number; service_date: string; number: number;
  status: TicketStatus; called_by: string | null; issued_at: string; called_at: string | null; closed_at: string | null;
  service_id?: string | null; service_name?: string | null;
};
export type QueueCommand = { type: 'issue'; service_id?: string } | { type: 'call_next' } | { type: 'complete'; ticket_id: string } | { type: 'cancel'; ticket_id: string };
export type QueueCommandResult = { queue_id: string; version: number; ticket: Ticket | null };
export type QueuePublicAccess = { public_url: string | null; qr_url: string | null; join_open: boolean; display_enabled: boolean; display_url: string | null; version: number };
export type QueueTipSettings = { recipient_id: string | null; recipient_name: string | null; public_slug: string | null; version: number };

export const roleLabels: Record<Role, string> = {
  owner: 'Владелец', manager: 'Управляющий', operator: 'Оператор', employee: 'Сотрудник',
};

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  constructor(status: number, code: string) {
    super(code);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
  }
}

export function readRecoveryInterval(error: unknown): number | false {
  if (!error) return false;
  if (error instanceof ApiError && error.status < 500 && ![408, 429].includes(error.status)) return false;
  return 5_000;
}

let signedOut = () => { window.location.replace('/login'); };
export function onSignedOut(callback: () => void) { signedOut = callback; }

function csrfToken(): string {
  const names = ['__Host-turnesta_csrf', 'turnesta_csrf'];
  for (const name of names) {
    const item = document.cookie.split(';').map((part) => part.trim()).find((part) => part.startsWith(`${name}=`));
    if (item) return item.slice(name.length + 1);
  }
  return '';
}

async function request<T>(url: string, options: RequestInit = {}): Promise<T> {
  const headers = new Headers(options.headers);
  if (options.method && options.method !== 'GET') {
    headers.set('X-CSRF-Token', csrfToken());
    if (options.body) headers.set('Content-Type', 'application/json');
  }
  const response = await fetch(url, { ...options, credentials: 'same-origin', headers });
  if (response.status === 401) {
    signedOut();
    throw new ApiError(401, 'unauthorized');
  }
  if (!response.ok) {
    const body: unknown = await response.json().catch(() => null);
    const code = typeof body === 'object' && body !== null && 'code' in body && typeof body.code === 'string'
      ? body.code : 'request_failed';
    throw new ApiError(response.status, code);
  }
  return response.status === 204 ? undefined as T : response.json() as Promise<T>;
}

function pageUrl(url: string, after: string | null): string {
  const query = new URLSearchParams({ limit: '50' });
  if (after) query.set('after', after);
  return `${url}?${query}`;
}

const organizationUrl = (tenant: string) => `/api/organizations/${encodeURIComponent(tenant)}`;
const queueUrl = (tenant: string, queue: string) => `${organizationUrl(tenant)}/queues/${encodeURIComponent(queue)}`;
export const api = {
  session: (signal: AbortSignal) => request<{ user_id: string }>('/auth/session', { signal }),
  logout: () => request<void>('/auth/logout', { method: 'POST' }),
  organizations: (after: string | null, signal: AbortSignal) => request<Page<OrganizationSummary>>(pageUrl('/api/organizations', after), { signal }),
  organization: (tenant: string, signal: AbortSignal) => request<Organization>(organizationUrl(tenant), { signal }),
  renameOrganization: (tenant: string, name: string, expected_version: number) => request<Organization>(organizationUrl(tenant), {
    method: 'PATCH', body: JSON.stringify({ name, expected_version }),
  }),
  locations: (tenant: string, after: string | null, signal: AbortSignal) => request<Page<Location>>(pageUrl(`${organizationUrl(tenant)}/locations`, after), { signal }),
  createLocation: (tenant: string, draft: LocationDraft) => request<Location>(`${organizationUrl(tenant)}/locations`, {
    method: 'POST', body: JSON.stringify(draft),
  }),
  updateLocation: (tenant: string, location: string, draft: LocationDraft, expected_version: number) => request<Location>(`${organizationUrl(tenant)}/locations/${encodeURIComponent(location)}`, {
    method: 'PATCH', body: JSON.stringify({ ...draft, expected_version }),
  }),
  timeZones: (query: string, signal: AbortSignal) => request<{ items: string[] }>(`/api/time-zones?${new URLSearchParams({ query, limit: '20' })}`, { signal }),
  tipRecipients: (tenant: string, after: string | null, signal: AbortSignal) => request<Page<TipRecipient>>(pageUrl(`${organizationUrl(tenant)}/tips/recipients`, after), { signal }),
  createTipRecipient: (tenant: string, draft: TipRecipientDraft) => request<TipRecipient>(`${organizationUrl(tenant)}/tips/recipients`, {
    method: 'POST', body: JSON.stringify(draft),
  }),
  tipAttempts: (tenant: string, after: string | null, signal: AbortSignal) => request<Page<TipAttempt>>(pageUrl(`${organizationUrl(tenant)}/tips/attempts`, after), { signal }),
  reconcileTipAttempt: (tenant: string, attempt: string) => request<TipAttempt>(`${organizationUrl(tenant)}/tips/attempts/${encodeURIComponent(attempt)}/reconcile`, { method: 'POST' }),
  queues: (tenant: string, location: string, after: string | null, signal: AbortSignal) => request<Page<Queue>>(pageUrl(`${organizationUrl(tenant)}/locations/${encodeURIComponent(location)}/queues`, after), { signal }),
  createQueue: (tenant: string, location: string, name: string) => request<Queue>(`${organizationUrl(tenant)}/locations/${encodeURIComponent(location)}/queues`, {
    method: 'POST', body: JSON.stringify({ name }),
  }),
  queue: (tenant: string, queue: string, signal: AbortSignal) => request<Queue>(queueUrl(tenant, queue), { signal }),
  queueEventsUrl: (tenant: string, queue: string) => `${queueUrl(tenant, queue)}/events`,
  queuePublicAccess: (tenant: string, queue: string, signal: AbortSignal) => request<QueuePublicAccess>(`${queueUrl(tenant, queue)}/public-access`, { signal }),
  queueServices: (tenant: string, queue: string, after: string | null, signal: AbortSignal) => request<Page<Service>>(pageUrl(`${queueUrl(tenant, queue)}/services`, after), { signal }),
  createService: (tenant: string, queue: string, draft: ServiceDraft) => request<Service>(`${queueUrl(tenant, queue)}/services`, { method: 'POST', body: JSON.stringify(draft) }),
  updateService: (tenant: string, queue: string, service: string, draft: ServiceDraft, expected_version: number) => request<Service>(`${queueUrl(tenant, queue)}/services/${encodeURIComponent(service)}`, { method: 'PATCH', body: JSON.stringify({ ...draft, expected_version }) }),
  queueTipSettings: (tenant: string, queue: string, signal: AbortSignal) => request<QueueTipSettings>(`${queueUrl(tenant, queue)}/tips`, { signal }),
  configureQueueTips: (tenant: string, queue: string, recipient_id: string | null, expected_version: number) => request<QueueTipSettings>(`${queueUrl(tenant, queue)}/tips`, {
    method: 'PUT', body: JSON.stringify({ recipient_id, expected_version }),
  }),
  configureQueuePublicAccess: (tenant: string, queue: string, join_open: boolean, expected_version: number) => request<QueuePublicAccess>(`${queueUrl(tenant, queue)}/public-access`, {
    method: 'PUT', body: JSON.stringify({ join_open, expected_version }),
  }),
  configureQueueDisplay: (tenant: string, queue: string, enabled: boolean, expected_version: number) => request<QueuePublicAccess>(`${queueUrl(tenant, queue)}/public-access/display`, {
    method: 'PUT', body: JSON.stringify({ enabled, expected_version }),
  }),
  tickets: (tenant: string, queue: string, status: TicketStatus, after: number | null, signal: AbortSignal, mine = false) => {
    const params = new URLSearchParams({ status, limit: '50' });
    if (after !== null) params.set('after', String(after));
    if (mine) params.set('mine', 'true');
    return request<Page<Ticket, number>>(`${queueUrl(tenant, queue)}/tickets?${params}`, { signal });
  },
  queueCommand: async (tenant: string, queue: string, key: string, command: QueueCommand) => {
    const result = await request<QueueCommandResult>(`${queueUrl(tenant, queue)}/commands`, {
      method: 'POST', headers: { 'Idempotency-Key': key }, body: JSON.stringify(command), signal: AbortSignal.timeout(10_000),
    });
    // A malformed successful response leaves the command outcome unknown.
    if (!result || result.queue_id !== queue || !Number.isSafeInteger(result.version) || result.version < 1
      || (result.ticket !== null && (!result.ticket || result.ticket.queue_id !== queue || typeof result.ticket.id !== 'string'
        || !Number.isSafeInteger(result.ticket.number) || result.ticket.number < 1 || typeof result.ticket.service_date !== 'string'
        || !['waiting', 'called', 'completed', 'cancelled'].includes(result.ticket.status)))
      || (command.type !== 'call_next' && result.ticket === null)
      || (('ticket_id' in command) && result.ticket?.id !== command.ticket_id)) {
      throw new Error('Invalid queue command response');
    }
    return result;
  },
};

export function errorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    if (error.code === 'tips_disabled') return 'Тестовый режим чаевых сейчас выключен. Обратитесь к администратору сервиса.';
    if (error.code === 'invalid_name') return 'Введите название от 1 до 120 символов без управляющих символов.';
    if (error.code === 'invalid_time_zone') return 'Такой часовой пояс не найден. Выберите значение из подсказок, например Europe/Moscow.';
    if (error.status === 403) return 'Недостаточно прав для этого действия или проверка безопасности не пройдена. Обновите страницу и попробуйте ещё раз.';
    if (error.status === 404) return 'Объект не найден или доступ к нему изменился.';
    if (error.status === 429) return 'Слишком много запросов. Подождите немного и повторите попытку.';
    if (error.status === 409) return 'Данные уже изменились. Загрузите актуальную версию и проверьте изменения.';
    if (error.status >= 500) return 'Сервис временно недоступен. Попробуйте ещё раз чуть позже.';
    if (error.status === 400) return 'Проверьте введённые данные и попробуйте ещё раз.';
  }
  return 'Не удалось связаться с сервером. Проверьте подключение и попробуйте ещё раз.';
}

export function validateName(value: string): string | undefined {
  const trimmed = value.trim();
  if (!trimmed) return 'Введите название.';
  if ([...trimmed].length > 120) return 'Название должно быть не длиннее 120 символов.';
  if (/[\u0000-\u001f\u007f-\u009f]/u.test(trimmed)) return 'Уберите управляющие символы из названия.';
  return undefined;
}
