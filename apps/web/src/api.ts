export type Role = 'owner' | 'manager' | 'operator' | 'employee';
export type OrganizationSummary = { id: string; name: string; role: Role };
export type Organization = { id: string; name: string; version: number };
export type Location = { id: string; tenant_id: string; name: string; time_zone: string; version: number };
export type Page<T> = { items: T[]; next_cursor: string | null };
export type LocationDraft = { name: string; time_zone: string };

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
};

export function errorMessage(error: unknown): string {
  if (error instanceof ApiError) {
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
