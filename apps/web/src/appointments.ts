import { request } from './api.ts';
import type { Page } from './api.ts';
import type { Offer } from './bookings.ts';

export type ClientInput = {
  client_name: string;
  phone: string;
  email: string;
  note: string;
};
export type Appointment = ClientInput & {
  id: string;
  specialist_id: string;
  offer_id: string;
  status: 'confirmed' | 'cancelled';
  source: 'staff';
  title: string;
  specialist_name: string;
  mode: Offer['mode'];
  date: string | null;
  starts_local: string | null;
  ends_local: string | null;
  starts_at: string | null;
  ends_at: string | null;
  time_zone: string;
  created_by: string;
  updated_by: string;
  created_at: string;
  updated_at: string;
  cancelled_at: string | null;
  version: number;
};
export type AppointmentCommand =
  | {
      type: 'record_appointment';
      id: string;
      specialist_id: string;
      offer_id: string;
      expected_offer_version: number;
      client: ClientInput;
    }
  | {
      type: 'update_client';
      id: string;
      specialist_id: string;
      expected_version: number;
      client: ClientInput;
    }
  | {
      type: 'move_appointment';
      id: string;
      specialist_id: string;
      offer_id: string;
      expected_offer_version: number;
      expected_version: number;
    }
  | {
      type: 'cancel_appointment';
      id: string;
      specialist_id: string;
      expected_version: number;
    };
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
export function isAppointmentAction(value: { type?: unknown }): boolean {
  return [
    'record_appointment',
    'update_client',
    'move_appointment',
    'cancel_appointment',
  ].includes(value.type as string);
}
export function validAppointmentCommand(
  value: Record<string, unknown>,
): value is Record<string, unknown> & AppointmentCommand {
  if (
    typeof value.id !== 'string' ||
    !uuid.test(value.id) ||
    typeof value.specialist_id !== 'string' ||
    !uuid.test(value.specialist_id)
  )
    return false;
  const positive = (n: unknown) => Number.isInteger(n) && (n as number) > 0;
  const keys = Object.keys(value).sort().join(',');
  const hasClient = () => {
    const c = value.client as Record<string, unknown> | null;
    return (
      c !== null &&
      typeof c === 'object' &&
      Object.keys(c).sort().join(',') === 'client_name,email,note,phone' &&
      ['client_name', 'phone', 'email', 'note'].every(
        (k) => typeof c[k] === 'string',
      )
    );
  };
  switch (value.type) {
    case 'record_appointment':
      return (
        keys ===
          'client,expected_offer_version,id,offer_id,specialist_id,type' &&
        typeof value.offer_id === 'string' &&
        uuid.test(value.offer_id) &&
        positive(value.expected_offer_version) &&
        hasClient()
      );
    case 'update_client':
      return (
        keys === 'client,expected_version,id,specialist_id,type' &&
        positive(value.expected_version) &&
        hasClient()
      );
    case 'move_appointment':
      return (
        keys ===
          'expected_offer_version,expected_version,id,offer_id,specialist_id,type' &&
        typeof value.offer_id === 'string' &&
        uuid.test(value.offer_id) &&
        positive(value.expected_offer_version) &&
        positive(value.expected_version)
      );
    case 'cancel_appointment':
      return (
        keys === 'expected_version,id,specialist_id,type' &&
        positive(value.expected_version)
      );
    default:
      return false;
  }
}
const base = (tenant: string, location: string) =>
  `/api/organizations/${encodeURIComponent(tenant)}/locations/${encodeURIComponent(location)}/bookings/appointments`;
export const appointmentApi = {
  list: (
    tenant: string,
    location: string,
    specialist: string,
    date: string,
    cancelled: boolean,
    after: string | null,
    signal: AbortSignal,
  ) =>
    request<Page<Appointment>>(
      `${base(tenant, location)}?${new URLSearchParams({ specialist_id: specialist, date, include_cancelled: String(cancelled), limit: '50', ...(after ? { after } : {}) })}`,
      { signal },
    ),
  get: (tenant: string, location: string, id: string, signal: AbortSignal) =>
    request<Appointment>(
      `${base(tenant, location)}/${encodeURIComponent(id)}`,
      { signal },
    ),
};
export function canRecord(offer: Offer, enabled: boolean): boolean {
  return (
    enabled &&
    offer.state === 'ready' &&
    offer.reserved_places !== null &&
    offer.reserved_places < offer.places &&
    offer.blocked_by_booking === false
  );
}
export function occupancyLabel(offer: Offer): string {
  if (offer.reserved_places === null || offer.reserved_places === undefined)
    return 'Занятость доступна оператору';
  if (offer.reserved_places >= offer.places)
    return offer.places === 1
      ? 'Клиент записан'
      : `Мест нет · ${offer.reserved_places} из ${offer.places}`;
  if (offer.blocked_by_booking) return 'Специалист занят в это время';
  return offer.places === 1
    ? 'Свободно'
    : `Свободно ${offer.places - offer.reserved_places} из ${offer.places}`;
}
