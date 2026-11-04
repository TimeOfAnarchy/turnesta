import { request } from './api.ts';
export type BookingFieldKind =
  | 'client_name'
  | 'phone'
  | 'email'
  | 'note'
  | 'text'
  | 'select';
export type BookingFormField = {
  key: string;
  label: string;
  kind: BookingFieldKind;
  required: boolean;
  options: string[];
};
export type BookingForm = { fields: BookingFormField[] };
export type BookingPublicSettings = {
  public_slug: string | null;
  public_url: string | null;
  enabled: boolean;
  privacy_url: string | null;
  form: BookingForm;
  version: number;
};
const base = (tenant: string, location: string) =>
  `/api/organizations/${encodeURIComponent(tenant)}/locations/${encodeURIComponent(location)}/bookings/public-access`;
export const publicBookingApi = {
  settings: (tenant: string, location: string, signal: AbortSignal) =>
    request<BookingPublicSettings>(base(tenant, location), { signal }),
  configure: (
    tenant: string,
    location: string,
    enabled: boolean,
    privacy_url: string,
    form: BookingForm,
    expected_version: number,
  ) =>
    request<BookingPublicSettings>(base(tenant, location), {
      method: 'PUT',
      signal: AbortSignal.timeout(10_000),
      body: JSON.stringify({ enabled, privacy_url, form, expected_version }),
    }),
};
export const fieldKindLabels: Record<BookingFieldKind, string> = {
  client_name: 'Имя',
  phone: 'Телефон',
  email: 'Email',
  note: 'Комментарий',
  text: 'Короткий текст',
  select: 'Выбор из списка',
};
export function newFormField(kind: BookingFieldKind): BookingFormField {
  return {
    key:
      kind === 'text' || kind === 'select'
        ? `field_${crypto.randomUUID().replaceAll('-', '')}`
        : kind,
    kind,
    label: kind === 'text' || kind === 'select' ? '' : fieldKindLabels[kind],
    required: kind === 'client_name',
    options: [],
  };
}
