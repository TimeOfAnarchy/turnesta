import { useState } from 'react';
import type { FormEvent } from 'react';
import { useInfiniteQuery } from '@tanstack/react-query';
import { ApiError, errorMessage, validateName } from './api';
import { ErrorNotice, Icon, Loading, Modal } from './components';
import {
  BookingCommands,
  bookingApi,
  bookingError,
  timeLabel,
  todayAt,
} from './bookings';
import type {
  BookingCommand,
  BookingReceipt,
  Offer,
  Specialist,
} from './bookings';
import { appointmentApi } from './appointments';
import type { Appointment, ClientInput } from './appointments';

type Controls = {
  busy: boolean;
  journal: BookingCommands;
  run: (command?: BookingCommand) => Promise<BookingReceipt>;
  close: () => void;
};
const emptyClient: ClientInput = {
  client_name: '',
  phone: '',
  email: '',
  note: '',
};
function clientOf(a: Appointment): ClientInput {
  return {
    client_name: a.client_name,
    phone: a.phone,
    email: a.email,
    note: a.note,
  };
}
function dateLabel(date: string | null) {
  return date
    ? new Date(`${date}T12:00:00Z`).toLocaleDateString('ru-RU', {
        day: 'numeric',
        month: 'long',
        year: 'numeric',
        timeZone: 'UTC',
      })
    : 'Дата по согласованию';
}
function ScheduleSummary({
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
function ClientFields({
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
function Failure({
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
function clientError(client: ClientInput) {
  return validateName(client.client_name) || undefined;
}

export function RecordAppointmentEditor({
  offer,
  current,
  specialist,
  busy,
  journal,
  run,
  close,
}: Controls & { offer: Offer; current?: Offer; specialist: Specialist }) {
  const [id] = useState(() => crypto.randomUUID());
  const [client, setClient] = useState<ClientInput>(emptyClient);
  const [version, setVersion] = useState(offer.version);
  const [error, setError] = useState<unknown>(null);
  const [invalid, setInvalid] = useState<string>();
  const changed = Boolean(current && current.version !== version);
  const unavailable = Boolean(
    current &&
      (current.state !== 'ready' ||
        (current.reserved_places !== null &&
          current.reserved_places >= current.places) ||
        current.blocked_by_booking),
  );
  async function save(command?: BookingCommand) {
    try {
      await run(command);
      close();
    } catch (e) {
      setError(e);
    }
  }
  function submit(e: FormEvent) {
    e.preventDefault();
    if (
      busy ||
      journal.pending ||
      changed ||
      unavailable ||
      !specialist.enabled
    )
      return;
    const err = clientError(client);
    setInvalid(err);
    if (!err)
      void save({
        type: 'record_appointment',
        id,
        specialist_id: specialist.id,
        offer_id: offer.id,
        expected_offer_version: version,
        client,
      });
  }
  return (
    <Modal
      title="Записать клиента"
      description="Запись сразу закрепит время или одно место в выбранном талоне."
      busy={busy}
      close={close}
    >
      <ScheduleSummary item={current ?? offer} name={specialist.name} />
      <form
        className="booking-form"
        onSubmit={submit}
        noValidate
        autoComplete="off"
      >
        <fieldset disabled={busy || Boolean(journal.pending)}>
          <ClientFields
            value={client}
            onChange={(v) => {
              setClient(v);
              setInvalid(undefined);
              setError(null);
            }}
          />
          {!offer.starts_local && !client.phone.trim() && (
            <p className="booking-limited-check">
              <Icon name="clock" />
              Время ещё не назначено. Телефон поможет договориться с клиентом;
              его можно оставить пустым.
            </p>
          )}
        </fieldset>
        {changed && (
          <div className="conflict-notice" role="alert">
            <p>
              Талон изменили. Проверьте новые сведения выше; данные клиента
              сохранены в форме.
            </p>
            <button
              type="button"
              className="text-button"
              disabled={busy || Boolean(journal.pending)}
              onClick={() => {
                setVersion(current!.version);
                setError(null);
              }}
            >
              Использовать актуальный талон
            </button>
          </div>
        )}
        {(unavailable || !specialist.enabled) && (
          <p className="conflict-notice" role="alert">
            Талон сейчас недоступен для новой записи. Закройте форму и выберите
            другое время.
          </p>
        )}
        {invalid && (
          <p className="field-error" role="alert">
            {invalid}
          </p>
        )}
        <Failure
          error={error}
          journal={journal}
          busy={busy}
          retry={() => void save()}
        />
        <div className="dialog-actions">
          <button
            className="button secondary"
            type="button"
            disabled={busy}
            onClick={close}
          >
            Закрыть
          </button>
          <button
            className="button primary"
            disabled={
              busy ||
              Boolean(journal.pending) ||
              changed ||
              unavailable ||
              !specialist.enabled
            }
          >
            {busy ? 'Записываем…' : 'Подтвердить запись'}
          </button>
        </div>
      </form>
    </Modal>
  );
}

export function AppointmentEditor({
  tenant,
  location,
  initial,
  busy,
  journal,
  run,
  close,
}: Controls & { tenant: string; location: string; initial: Appointment }) {
  const [item, setItem] = useState(initial);
  const [client, setClient] = useState(() => clientOf(initial));
  const [mode, setMode] = useState<'view' | 'edit' | 'move' | 'cancel'>('view');
  const [date, setDate] = useState(initial.date ?? todayAt(initial.time_zone));
  const [target, setTarget] = useState<Offer | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [invalid, setInvalid] = useState<string>();
  const [loading, setLoading] = useState(false);
  const offers = useInfiniteQuery({
    queryKey: [
      'organization',
      tenant,
      'bookings',
      location,
      'move-offers',
      item.specialist_id,
      date,
    ],
    initialPageParam: null as string | null,
    enabled: mode === 'move',
    queryFn: ({ pageParam, signal }) =>
      bookingApi.offers(
        tenant,
        location,
        item.specialist_id,
        date,
        false,
        pageParam,
        signal,
      ),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    retry: false,
    refetchInterval: 10_000,
  });
  const options = [
    ...new Map(
      (offers.data?.pages.flatMap((p) => p.items) ?? [])
        .filter((o) => o.id !== item.offer_id && o.state === 'ready')
        .map((o) => [o.id, o]),
    ).values(),
  ].sort(
    (a, b) =>
      (a.starts_local ?? 'z').localeCompare(b.starts_local ?? 'z') ||
      a.id.localeCompare(b.id),
  );
  const freshTarget = target
    ? options.find((o) => o.id === target.id)
    : undefined;
  const staleTarget = Boolean(
    target && (!freshTarget || freshTarget.version !== target.version),
  );
  const blocked = busy || Boolean(journal.pending) || loading;
  function changeMode(value: typeof mode) {
    setMode(value);
    setError(null);
    setInvalid(undefined);
    setTarget(null);
  }
  async function refresh() {
    setLoading(true);
    try {
      const fresh = await appointmentApi.get(
        tenant,
        location,
        item.id,
        AbortSignal.timeout(10_000),
      );
      setItem(fresh);
      setClient(clientOf(fresh));
      setMode('view');
      setError(null);
    } catch (e) {
      setError(e);
    } finally {
      setLoading(false);
    }
  }
  async function save(command?: BookingCommand) {
    try {
      await run(command);
      close();
    } catch (e) {
      setError(e);
    }
  }
  function submit(e: FormEvent) {
    e.preventDefault();
    if (blocked || item.status !== 'confirmed') return;
    if (mode === 'edit') {
      const err = clientError(client);
      setInvalid(err);
      if (!err)
        void save({
          type: 'update_client',
          id: item.id,
          specialist_id: item.specialist_id,
          expected_version: item.version,
          client,
        });
    }
    if (mode === 'move' && target && !staleTarget)
      void save({
        type: 'move_appointment',
        id: item.id,
        specialist_id: item.specialist_id,
        expected_version: item.version,
        offer_id: target.id,
        expected_offer_version: target.version,
      });
    if (mode === 'cancel')
      void save({
        type: 'cancel_appointment',
        id: item.id,
        specialist_id: item.specialist_id,
        expected_version: item.version,
      });
  }
  return (
    <Modal
      title={
        mode === 'cancel'
          ? 'Отменить запись?'
          : mode === 'move'
            ? 'Перенести запись'
            : mode === 'edit'
              ? 'Данные клиента'
              : 'Запись клиента'
      }
      description={`${item.client_name} · ${item.status === 'confirmed' ? 'Запись подтверждена' : 'Запись отменена'}`}
      busy={busy || loading}
      close={close}
    >
      <ScheduleSummary item={item} name={item.specialist_name} />
      <form
        className="booking-form"
        noValidate
        autoComplete="off"
        onSubmit={submit}
      >
        <fieldset disabled={blocked}>
          {mode === 'view' && (
            <>
              <dl className="appointment-contact">
                <dt>Имя</dt>
                <dd>{item.client_name}</dd>
                <dt>Телефон</dt>
                <dd>{item.phone || 'Не указан'}</dd>
                <dt>Email</dt>
                <dd>{item.email || 'Не указан'}</dd>
                {item.note && (
                  <>
                    <dt>Комментарий</dt>
                    <dd className="appointment-note">{item.note}</dd>
                  </>
                )}
                {item.form_snapshot?.fields
                  .filter(
                    (field) => field.kind === 'text' || field.kind === 'select',
                  )
                  .map((field) => (
                    <div className="appointment-extra-answer" key={field.key}>
                      <dt>{field.label}</dt>
                      <dd className="appointment-note">
                        {item.answers?.[field.key] || 'Не указано'}
                      </dd>
                    </div>
                  ))}
              </dl>
              <p className="field-hint">
                {item.source === 'public'
                  ? 'Онлайн-запись'
                  : 'Создана сотрудником'}{' '}
                ·{' '}
                {new Date(item.created_at).toLocaleString('ru-RU', {
                  timeZone: item.time_zone,
                })}
              </p>
              {item.cancelled_at && (
                <p className="conflict-notice">
                  Отменена ·{' '}
                  {new Date(item.cancelled_at).toLocaleString('ru-RU', {
                    timeZone: item.time_zone,
                  })}
                </p>
              )}
              {item.status === 'confirmed' && (
                <div className="appointment-controls">
                  <button
                    type="button"
                    className="button secondary"
                    onClick={() => changeMode('edit')}
                  >
                    Изменить данные
                  </button>
                  <button
                    type="button"
                    className="button secondary"
                    onClick={() => changeMode('move')}
                  >
                    Перенести
                  </button>
                  <button
                    type="button"
                    className="text-button danger"
                    onClick={() => changeMode('cancel')}
                  >
                    Отменить запись
                  </button>
                </div>
              )}
            </>
          )}
          {mode === 'edit' && (
            <ClientFields
              value={client}
              onChange={(v) => {
                setClient(v);
                setError(null);
                setInvalid(undefined);
              }}
            />
          )}
          {mode === 'cancel' && (
            <div className="conflict-notice">
              <strong>Место освободится для другой записи.</strong>
              <p>
                Данные этой записи сохранятся в истории отменённых. Уведомление
                клиенту не отправляется.
              </p>
            </div>
          )}
          {mode === 'move' && (
            <>
              <p className="field-hint">
                Выберите другой талон у этого специалиста. Текущая запись
                освободит прежнее время только после успешного переноса.
              </p>
              <div className="form-field">
                <label htmlFor="appointment-move-date">
                  Дата нового талона
                </label>
                <input
                  id="appointment-move-date"
                  type="date"
                  value={date}
                  onChange={(e) => {
                    setDate(e.target.value);
                    setTarget(null);
                    setError(null);
                  }}
                />
              </div>
              {offers.isPending ? (
                <Loading label="Загружаем талоны…" />
              ) : offers.isError ? (
                <ErrorNotice
                  error={offers.error}
                  retry={() => void offers.refetch()}
                />
              ) : (
                <>
                  <div
                    className="appointment-targets"
                    role="group"
                    aria-label="Новый талон"
                  >
                    {options.map((o) => (
                      <button
                        type="button"
                        className="appointment-target"
                        aria-pressed={target?.id === o.id}
                        key={o.id}
                        onClick={() => {
                          setTarget(o);
                          setError(null);
                        }}
                      >
                        <strong>{timeLabel(o)}</strong>
                        <span>{o.title || 'Приём'}</span>
                        <small>
                          {o.date ? dateLabel(o.date) : 'Дата по согласованию'}{' '}
                          · {o.time_zone}
                        </small>
                      </button>
                    ))}
                  </div>
                  {!options.length && (
                    <p className="field-hint">
                      Других талонов на эту дату нет. Выберите другую дату.
                    </p>
                  )}
                  {offers.hasNextPage && (
                    <button
                      type="button"
                      className="text-button"
                      disabled={offers.isFetching}
                      onClick={() => void offers.fetchNextPage()}
                    >
                      Ещё талоны
                    </button>
                  )}
                </>
              )}
              <p className="field-hint">
                Занятость проверится при сохранении, с учётом освобождения
                текущего времени. Перенос к другому специалисту добавим
                отдельным шагом.
              </p>
              {staleTarget && (
                <p className="conflict-notice" role="alert">
                  Выбранный талон изменился. Выберите его заново после проверки.
                </p>
              )}
            </>
          )}
        </fieldset>
        {invalid && (
          <p className="field-error" role="alert">
            {invalid}
          </p>
        )}
        <Failure
          error={error}
          journal={journal}
          busy={busy}
          retry={() => void save()}
        />
        {error instanceof ApiError &&
          ['version_conflict', 'appointment_closed'].includes(error.code) &&
          !journal.pending && (
            <button
              className="text-button"
              type="button"
              disabled={blocked}
              onClick={() => void refresh()}
            >
              Загрузить сохранённую версию записи
            </button>
          )}
        <div className="dialog-actions">
          {mode !== 'view' && (
            <button
              className="button secondary"
              type="button"
              disabled={blocked}
              onClick={() => changeMode('view')}
            >
              Назад
            </button>
          )}
          <button
            className="button secondary"
            type="button"
            disabled={busy || loading}
            onClick={close}
          >
            Закрыть
          </button>
          {mode !== 'view' && (
            <button
              className={`button ${mode === 'cancel' ? 'appointment-cancel' : 'primary'}`}
              disabled={
                blocked ||
                item.status !== 'confirmed' ||
                (mode === 'move' && (!target || staleTarget))
              }
            >
              {busy
                ? 'Сохраняем…'
                : mode === 'cancel'
                  ? 'Да, отменить запись'
                  : mode === 'move'
                    ? 'Подтвердить перенос'
                    : 'Сохранить данные'}
            </button>
          )}
        </div>
      </form>
    </Modal>
  );
}

export function AppointmentsPanel({
  tenant,
  location,
  specialist,
  date,
  open,
  disabled,
}: {
  tenant: string;
  location: string;
  specialist: string;
  date: string;
  open: (item: Appointment) => void;
  disabled: boolean;
}) {
  const [cancelled, setCancelled] = useState(false);
  const query = useInfiniteQuery({
    queryKey: [
      'organization',
      tenant,
      'bookings',
      location,
      'appointments',
      specialist,
      date,
      cancelled,
    ],
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) =>
      appointmentApi.list(
        tenant,
        location,
        specialist,
        date,
        cancelled,
        pageParam,
        signal,
      ),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    retry: false,
    refetchInterval: 10_000,
  });
  const items = [
    ...new Map(
      (query.data?.pages.flatMap((p) => p.items) ?? []).map((a) => [a.id, a]),
    ).values(),
  ].sort(
    (a, b) =>
      (a.starts_local ?? 'z').localeCompare(b.starts_local ?? 'z') ||
      a.client_name.localeCompare(b.client_name),
  );
  return (
    <section
      className="booking-section appointment-section"
      aria-labelledby="appointments-title"
    >
      <div className="section-line">
        <div>
          <p className="small-eyebrow">КЛИЕНТЫ</p>
          <h2 id="appointments-title">Записи клиентов</h2>
        </div>
        <label className="checkbox-label">
          <input
            type="checkbox"
            checked={cancelled}
            onChange={(e) => setCancelled(e.target.checked)}
          />
          Показать отменённые
        </label>
      </div>
      {query.isPending ? (
        <Loading label="Загружаем записи…" />
      ) : query.isError ? (
        <ErrorNotice error={query.error} retry={() => void query.refetch()} />
      ) : !items.length ? (
        <div className="panel appointment-empty">
          <Icon name="users" />
          <div>
            <strong>Записей пока нет</strong>
            <p>Выберите свободный талон выше и нажмите «Записать клиента».</p>
          </div>
        </div>
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
