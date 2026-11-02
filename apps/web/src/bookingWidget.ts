import { request } from './api.ts';
export type WidgetSettings = {
  allowed_origins: string[];
  version: number;
  public_url: string | null;
  script_url: string;
};
const base = (tenant: string, point: string) =>
  `/api/organizations/${encodeURIComponent(tenant)}/locations/${encodeURIComponent(point)}/bookings/widget`;
export const bookingWidgetApi = {
  settings: (tenant: string, point: string, signal: AbortSignal) =>
    request<WidgetSettings>(base(tenant, point), { signal }),
  configure: (
    tenant: string,
    point: string,
    allowed_origins: string[],
    expected_version: number,
  ) =>
    request<WidgetSettings>(base(tenant, point), {
      method: 'PUT',
      signal: AbortSignal.timeout(10_000),
      body: JSON.stringify({ allowed_origins, expected_version }),
    }),
};
function escapeAttribute(value: string) {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('"', '&quot;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;');
}
export function widgetSnippet(publicUrl: string, scriptUrl: string) {
  const url = escapeAttribute(publicUrl),
    script = escapeAttribute(scriptUrl);
  return `<div data-turnesta-booking="${url}">\n  <a href="${url}" target="_blank" rel="noopener noreferrer">Записаться онлайн</a>\n</div>\n<script src="${script}" defer referrerpolicy="no-referrer"></script>`;
}
