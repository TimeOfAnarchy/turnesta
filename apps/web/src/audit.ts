import { request } from './api.ts';

export type AuditEntry = {
  id: string;
  actor_id: string;
  actor_name: string;
  location_id: string;
  location_name: string;
  action: 'list' | 'detail' | 'command' | 'export_json' | 'export_csv';
  appointment_id: string | null;
  specialist_id: string | null;
  specialist_name: string | null;
  visit_date: string | null;
  after_id: string | null;
  page_limit: number | null;
  include_cancelled: boolean | null;
  result_count: number | null;
  created_at: string;
};
export type AuditCursor = { before_at: string; before_id: string };
export type AuditPage = { items: AuditEntry[]; next_cursor: AuditCursor | null };
export type AuditSettings = { retention_days: number; version: number };
export type AuditFilters = { from: string; to: string; actor: string };
export function formatAuditTime(instant: string) {
  return new Date(instant).toLocaleString('ru-RU', { timeZone: 'UTC' }) + ' UTC';
}
const base = (tenant: string) => `/api/organizations/${encodeURIComponent(tenant)}/audit`;
export const auditApi = {
  list: (tenant: string, filters: AuditFilters, cursor: AuditCursor | null, signal: AbortSignal) => {
    const query = new URLSearchParams({ limit: '50' });
    for (const [key, value] of Object.entries(filters)) if (value) query.set(key, value);
    if (cursor) { query.set('before_at', cursor.before_at); query.set('before_id', cursor.before_id); }
    return request<AuditPage>(`${base(tenant)}/access?${query}`, { signal });
  },
  settings: (tenant: string, signal: AbortSignal) => request<AuditSettings>(`${base(tenant)}/settings`, { signal }),
  configure: (tenant: string, retention_days: number, expected_version: number) =>
    request<AuditSettings>(`${base(tenant)}/settings`, { method: 'PUT', body: JSON.stringify({ retention_days, expected_version }), signal: AbortSignal.timeout(10_000) }),
};
