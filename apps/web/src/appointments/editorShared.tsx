import { errorMessage, validateName } from '../api';
import type { Appointment, ClientInput } from '../appointments';
import type { BookingCommand, BookingReceipt, Offer } from '../bookings';
import { BookingCommands, bookingError, timeLabel } from '../bookings';
import { Icon } from '../components';
export type Controls = {
  busy: boolean;
  journal: BookingCommands;
  run: (command?: BookingCommand) => Promise<BookingReceipt>;
  close: () => void;
};
export const emptyClient: ClientInput = {
  client_name: '',
  phone: '',
  email: '',
  note: '',
};
export function clientOf(a: Appointment): ClientInput {
  return {
    client_name: a.client_name,
    phone: a.phone,
    email: a.email,
    note: a.note,
  };
}
export function dateLabel(date: string | null) {
  return date
    ? new Date(`${date}T12:00:00Z`).toLocaleDateString('ru-RU', {
        day: 'numeric',
        month: 'long',
        year: 'numeric',
        timeZone: 'UTC',
      })
    : 'Дата по согласованию';
}
export function ScheduleSummary({
  item,
  name,
}: {
  item: Pick<
    Offer,
    'date' | 'starts_local' | 'ends_local' | 'time_zone' | 'title'
  >;
  name: string;
}) {
  return (
    <div className="appointment-summary">
      <Icon name="calendar" />
      <div>
        <strong>
          {dateLabel(item.date)} · {timeLabel(item)}
        </strong>
        <p>
          {name} · {item.title || 'Приём'}
        </p>
        <small>{item.time_zone}</small>
      </div>
    </div>
  );
}
export function ClientFields({
  value,
  onChange,
}: {
  value: ClientInput;
  onChange: (value: ClientInput) => void;
}) {
  const update = (key: keyof ClientInput, text: string) =>
    onChange({ ...value, [key]: text });
  return (
    <>
      <div className="form-field">
        <label htmlFor="appointment-name">Имя клиента</label>
        <input
          id="appointment-name"
          value={value.client_name}
          onChange={(e) => update('client_name', e.target.value)}
          maxLength={120}
          autoComplete="off"
          required
        />
      </div>
      <div className="booking-form-grid">
        <div className="form-field">
          <label htmlFor="appointment-phone">
            Телефон <span className="subtle">· необязательно</span>
          </label>
          <input
            id="appointment-phone"
            type="tel"
            value={value.phone}
            onChange={(e) => update('phone', e.target.value)}
            maxLength={64}
            autoComplete="off"
          />
        </div>
        <div className="form-field">
          <label htmlFor="appointment-email">
            Email <span className="subtle">· необязательно</span>
          </label>
          <input
            id="appointment-email"
            type="email"
            value={value.email}
            onChange={(e) => update('email', e.target.value)}
            maxLength={254}
            autoComplete="off"
          />
        </div>
      </div>
      <div className="form-field">
        <label htmlFor="appointment-note">
          Комментарий к записи <span className="subtle">· необязательно</span>
        </label>
        <textarea
          id="appointment-note"
          rows={3}
          maxLength={1000}
          value={value.note}
          onChange={(e) => update('note', e.target.value)}
        />
        <p className="field-hint">
          Контакты и комментарий видны только сотрудникам с правом работы с
          записями. Уведомления пока не отправляются.
        </p>
      </div>
    </>
  );
}
export function Failure({
  error,
  journal,
  busy,
  retry,
}: {
  error: unknown;
  journal: BookingCommands;
  busy: boolean;
  retry: () => void;
}) {
  return (
    <>
      {error !== null && (
        <p className="field-error" role="alert">
          {bookingError(error) ?? errorMessage(error)}
        </p>
      )}
      {journal.pending && !busy && (
        <div className="conflict-notice" role="alert">
          <p>
            Результат не подтверждён. Восстановите прежний запрос перед новым
            сохранением.
          </p>
          <button type="button" className="button secondary" onClick={retry}>
            Повторить прежний запрос
          </button>
        </div>
      )}
    </>
  );
}
export function clientError(client: ClientInput) {
  return validateName(client.client_name) || undefined;
}
