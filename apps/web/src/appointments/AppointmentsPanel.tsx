import type { AppointmentSummary } from '../appointments';
import { timeLabel } from '../bookings';
import { ErrorNotice, Loading } from '../components';
import { dateLabel } from './editorShared';
import type { useDayAppointments } from './useDayAppointments';
// Confirmed appointments of visible slots are shown inside the slot cards;
// this panel keeps the rest: cancelled history and records outside the list.
export function AppointmentsPanel({
  query,
  placed,
  cancelled,
  open,
  disabled,
}: {
  query: ReturnType<typeof useDayAppointments>;
  placed: ReadonlySet<string>;
  cancelled: boolean;
  open: (item: AppointmentSummary) => void;
  disabled: boolean;
}) {
  const items = [
    ...new Map(
      (query.data?.pages.flatMap((p) => p.items) ?? []).map((a) => [a.id, a]),
    ).values(),
  ]
    .filter((a) => a.status === 'cancelled' || !placed.has(a.id))
    .sort(
      (a, b) =>
        (a.starts_local ?? 'z').localeCompare(b.starts_local ?? 'z') ||
        a.client_name.localeCompare(b.client_name),
    );
  if (!cancelled && query.isSuccess && !items.length && !query.hasNextPage)
    return null;
  return (
    <section
      className="booking-section appointment-section"
      aria-labelledby="appointments-title"
    >
      <div className="section-line">
        <h2 id="appointments-title">
          {cancelled ? 'Отменённые и другие записи' : 'Другие записи дня'}
        </h2>
      </div>
      {query.isPending ? (
        <Loading label="Загружаем записи…" />
      ) : query.isError ? (
        <ErrorNotice error={query.error} retry={() => void query.refetch()} />
      ) : !items.length ? (
        <p className="appointment-empty">
          Отменённых записей на этот день нет.
        </p>
      ) : (
        <div className="appointment-list">
          {items.map((a) => (
            <article
              className={`appointment-row appointment-${a.status}`}
              key={a.id}
            >
              <div className="appointment-row-time">
                <strong>{timeLabel(a)}</strong>
                <small>
                  {a.date ? dateLabel(a.date) : 'Дата по согласованию'}
                </small>
              </div>
              <div className="appointment-row-client">
                <h3>{a.client_name}</h3>
                <p>
                  {a.phone || 'Телефон не указан'} · {a.title || 'Приём'}
                </p>
              </div>
              <span className="booking-state">
                {a.status === 'confirmed' ? 'Подтверждена' : 'Отменена'}
              </span>
              <button
                className="text-button"
                type="button"
                disabled={disabled}
                aria-label={`Открыть запись ${a.client_name} ${timeLabel(a)}`}
                onClick={() => open(a)}
              >
                Открыть запись
              </button>
            </article>
          ))}
        </div>
      )}
      {query.hasNextPage && (
        <button
          type="button"
          className="button secondary"
          disabled={query.isFetching}
          onClick={() => void query.fetchNextPage()}
        >
          Ещё записи
        </button>
      )}
    </section>
  );
}
