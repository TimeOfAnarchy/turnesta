import { useEffect, useState } from 'react';
import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link, useSearchParams } from 'react-router-dom';
import { useWorkspace } from './App';
import { ApiError, errorMessage } from './api';
import { ErrorNotice, Icon, Loading, SuccessNotice } from './components';
import { keys, useLocations } from './queries';
import { OfferEditor, CatalogEditor } from './BookingEditors';
import { BookingsHelp } from './BookingsHelp';
import { PublicBookingSettings } from './PublicBookingSettings';
import { BookingBatchEditor } from './BookingBatchEditor';
import {
  RecordAppointmentEditor,
  AppointmentEditor,
  AppointmentsPanel,
  useDayAppointments,
} from './AppointmentEditors';
import { canRecord, occupancyLabel, isAppointmentAction } from './appointments';
import type { Appointment } from './appointments';
import {
  bookingApi,
  BookingCommands,
  bookingError,
  todayAt,
  shiftDate,
  timeLabel,
  offerStateLabels,
} from './bookings';
import type {
  Specialist,
  Preset,
  Offer,
  BookingCommand,
  BookingReceipt,
} from './bookings';

const bookingKey = (tenant: string, location: string) =>
  ['organization', tenant, 'bookings', location] as const;
function storage() {
  try {
    return window.sessionStorage;
  } catch {
    return null;
  }
}
function distinct<T extends { id: string }>(items: T[]) {
  return [...new Map(items.map((i) => [i.id, i])).values()];
}
type View = 'schedule' | 'online' | 'catalog' | 'help';
const views: {
  id: View;
  label: string;
  extra?: string;
  icon: 'calendar' | 'globe' | 'users' | 'help';
}[] = [
  { id: 'schedule', label: 'Расписание', icon: 'calendar' },
  { id: 'online', label: 'Онлайн-запись', icon: 'globe' },
  { id: 'catalog', label: 'Специалисты', extra: ' и заготовки', icon: 'users' },
  { id: 'help', label: 'Справка', icon: 'help' },
];
function readView(value: string | null): View {
  return value === 'online' || value === 'catalog' || value === 'help'
    ? value
    : 'schedule';
}
export function BookingsPage() {
  const { organization, canManage } = useWorkspace();
  const locations = useLocations(organization.id);
  const [params, setParams] = useSearchParams();
  const items = distinct(locations.data?.pages.flatMap((p) => p.items) ?? []);
  const location = params.get('location') ?? items[0]?.id ?? '';
  const point = items.find((i) => i.id === location);
  const view = readView(params.get('view'));
  function search(next: { location?: string; view?: View }) {
    const value = new URLSearchParams();
    const nextLocation = next.location ?? params.get('location');
    const nextView = next.view ?? view;
    if (nextLocation) value.set('location', nextLocation);
    if (nextView !== 'schedule') value.set('view', nextView);
    return value;
  }
  useEffect(() => {
    document.title = `Запись · ${organization.name} · Turnesta`;
  }, [organization.name]);
  useEffect(() => {
    if (
      location &&
      !point &&
      locations.hasNextPage &&
      !locations.isFetching &&
      !locations.isError
    )
      void locations.fetchNextPage();
  }, [
    location,
    point,
    locations.hasNextPage,
    locations.isFetching,
    locations.isError,
    locations.fetchNextPage,
  ]);
  const singlePoint = point && items.length === 1 && !locations.hasNextPage;
  return (
    <>
      <div className="page-heading booking-page-heading">
        <div>
          <p className="eyebrow">ОРГАНИЗАЦИЯ / ЗАПИСЬ</p>
          <h1>
            Запись клиентов<span className="heading-dot">.</span>
          </h1>
          <p className="page-intro">
            Расписание приёмов, записи клиентов и онлайн-запись — в одном
            месте.
          </p>
        </div>
      </div>
      {locations.isPending ? (
        <Loading label="Загружаем точки…" />
      ) : locations.isError ? (
        <ErrorNotice
          error={locations.error}
          retry={() => {
            void locations.refetch();
          }}
        />
      ) : !items.length ? (
        <div className="panel empty-state">
          <span className="large-icon">
            <Icon name="locations" />
          </span>
          <h2>Сначала добавьте точку</h2>
          <p>Специалисты и расписание относятся к месту приёма.</p>
          <Link
            className="button primary"
            to={`/organizations/${organization.id}/locations?new=1&back=bookings`}
          >
            <Icon name="plus" />
            Добавить точку
          </Link>
        </div>
      ) : (
        <>
          <div className="booking-toolbar">
            <nav className="booking-tabs" aria-label="Разделы записи">
              {views.map((item) => (
                <Link
                  key={item.id}
                  to={{ search: search({ view: item.id }).toString() }}
                  aria-current={item.id === view ? 'page' : undefined}
                  replace
                >
                  <Icon name={item.icon} />
                  <span>
                    {item.label}
                    {item.extra && (
                      <span className="booking-tab-extra">{item.extra}</span>
                    )}
                  </span>
                </Link>
              ))}
            </nav>
            <div className="booking-location">
              {singlePoint ? (
                <span className="booking-point">
                  <Icon name="locations" />
                  <span>
                    <strong>{point.name}</strong>
                    <small>Время точки · {point.time_zone}</small>
                  </span>
                </span>
              ) : (
                <>
                  <div className="form-field">
                    <label htmlFor="booking-location">Точка приёма</label>
                    <select
                      id="booking-location"
                      value={location}
                      onChange={(e) =>
                        setParams(search({ location: e.target.value }))
                      }
                    >
                      {!point && (
                        <option value={location}>
                          Загружаем выбранную точку…
                        </option>
                      )}
                      {items.map((i) => (
                        <option key={i.id} value={i.id}>
                          {i.name}
                        </option>
                      ))}
                    </select>
                  </div>
                  {point && (
                    <span className="booking-zone">
                      <Icon name="globe" />
                      Время точки · {point.time_zone}
                    </span>
                  )}
                  {locations.hasNextPage && (
                    <button
                      className="text-button"
                      type="button"
                      disabled={locations.isFetching}
                      onClick={() => {
                        void locations.fetchNextPage();
                      }}
                    >
                      Ещё точки
                    </button>
                  )}
                </>
              )}
            </div>
          </div>
          {point &&
            (view === 'help' ? (
              <BookingsHelp />
            ) : view === 'online' ? (
              <PublicBookingSettings
                key={`${organization.id}:${location}`}
                tenant={organization.id}
                location={location}
                canManage={canManage}
              />
            ) : (
              <Schedule
                key={`${organization.id}:${location}`}
                tenant={organization.id}
                location={location}
                zone={point.time_zone}
                view={view}
              />
            ))}
        </>
      )}
    </>
  );
}
type Editor =
  | { kind: 'batch'; specialist: Specialist }
  | { kind: 'record'; offer: Offer; specialist: Specialist }
  | { kind: 'appointment'; item: Appointment }
  | { kind: 'specialist'; item: Specialist | null }
  | { kind: 'preset'; item: Preset | null; back?: Specialist }
  | {
      kind: 'offer';
      item: Offer | null;
      copy?: boolean;
      specialist: Specialist;
    };
function dayTitle(date: string, today: string) {
  const label = new Intl.DateTimeFormat('ru-RU', {
    weekday: 'long',
    day: 'numeric',
    month: 'long',
    timeZone: 'UTC',
  }).format(new Date(`${date}T12:00:00Z`));
  const shift = Math.round(
    (Date.parse(`${date}T00:00:00Z`) - Date.parse(`${today}T00:00:00Z`)) /
      86_400_000,
  );
  const relative = { [-1]: 'Вчера', 0: 'Сегодня', 1: 'Завтра' }[shift];
  return relative ? `${relative}, ${label}` : label;
}
function Schedule({
  tenant,
  location,
  zone,
  view,
}: {
  tenant: string;
  location: string;
  zone: string;
  view: 'schedule' | 'catalog';
}) {
  const { canManage, userId, role } = useWorkspace();
  const canBook = role !== 'employee';
  const client = useQueryClient();
  const base = bookingKey(tenant, location);
  const profiles = useInfiniteQuery({
    queryKey: [...base, 'specialists'],
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) =>
      bookingApi.specialists(tenant, location, pageParam, signal),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    retry: false,
    refetchInterval: 10_000,
  });
  const presets = useInfiniteQuery({
    queryKey: [...base, 'presets'],
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) =>
      bookingApi.presets(tenant, location, pageParam, signal),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    retry: false,
    refetchInterval: 10_000,
  });
  const specialists = distinct(
    profiles.data?.pages.flatMap((p) => p.items) ?? [],
  );
  const templates = distinct(presets.data?.pages.flatMap((p) => p.items) ?? []);
  const [chosen, setChosen] = useState('');
  const selected = specialists.find(
    (i) =>
      i.id ===
      (chosen || specialists.find((s) => s.enabled)?.id || specialists[0]?.id),
  );
  useEffect(() => {
    if (!chosen && specialists.length)
      setChosen(specialists.find((s) => s.enabled)?.id ?? specialists[0].id);
  }, [chosen, specialists]);
  useEffect(() => {
    if (
      chosen &&
      !specialists.some((s) => s.id === chosen) &&
      profiles.hasNextPage &&
      !profiles.isFetching &&
      !profiles.isError
    )
      void profiles.fetchNextPage();
  }, [
    chosen,
    specialists,
    profiles.hasNextPage,
    profiles.isFetching,
    profiles.isError,
    profiles.fetchNextPage,
  ]);
  const today = todayAt(zone);
  const [date, setDate] = useState(today);
  const [withdrawn, setWithdrawn] = useState(false);
  const [cancelled, setCancelled] = useState(false);
  const offers = useInfiniteQuery({
    queryKey: [...base, 'offers', selected?.id, date, withdrawn],
    initialPageParam: null as string | null,
    enabled: Boolean(selected) && view === 'schedule',
    queryFn: ({ pageParam, signal }) =>
      bookingApi.offers(
        tenant,
        location,
        selected!.id,
        date,
        withdrawn,
        pageParam,
        signal,
      ),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    retry: false,
    refetchInterval: 10_000,
  });
  const appointments = useDayAppointments(
    tenant,
    location,
    selected?.id ?? '',
    date,
    false,
    canBook && Boolean(selected) && view === 'schedule',
  );
  const items = distinct(offers.data?.pages.flatMap((p) => p.items) ?? []);
  // An empty day points to the nearest scheduled day within two weeks.
  const nextDay = useQuery({
    queryKey: [...base, 'next-day', selected?.id, date],
    enabled:
      Boolean(selected) &&
      view === 'schedule' &&
      offers.isSuccess &&
      !items.length &&
      !withdrawn,
    staleTime: 60_000,
    retry: false,
    queryFn: async ({ signal }) => {
      for (let step = 1; step <= 14; step++) {
        const day = shiftDate(date, step);
        const page = await bookingApi.offers(
          tenant,
          location,
          selected!.id,
          day,
          false,
          null,
          signal,
        );
        const ready = page.items.filter((i) => i.state === 'ready');
        if (ready.length)
          return {
            date: day,
            count: ready.length,
            booked: ready.reduce(
              (sum, i) => sum + Math.min(i.reserved_places ?? 0, i.places),
              0,
            ),
          };
      }
      return null;
    },
  });
  const timed = items
    .filter((i) => i.mode !== 'by_agreement')
    .sort(
      (a, b) =>
        (a.starts_local ?? '').localeCompare(b.starts_local ?? '') ||
        a.id.localeCompare(b.id),
    );
  const agreement = items
    .filter((i) => i.mode === 'by_agreement')
    .sort(
      (a, b) =>
        Number(a.date === null) - Number(b.date === null) ||
        a.id.localeCompare(b.id),
    );
  const visible = new Set(items.map((i) => i.id));
  const clients = new Map<string, Appointment[]>();
  const placed = new Set<string>();
  for (const a of appointments.data?.pages.flatMap((p) => p.items) ?? []) {
    if (a.status !== 'confirmed' || !visible.has(a.offer_id) || placed.has(a.id))
      continue;
    placed.add(a.id);
    clients.set(a.offer_id, [...(clients.get(a.offer_id) ?? []), a]);
  }
  const ready = items.filter((i) => i.state === 'ready');
  const booked = ready.reduce(
    (sum, i) => sum + Math.min(i.reserved_places ?? 0, i.places),
    0,
  );
  const free = ready.reduce(
    (sum, i) =>
      sum +
      (i.reserved_places === null || i.blocked_by_booking
        ? 0
        : Math.max(i.places - i.reserved_places, 0)),
    0,
  );
  const [journal] = useState(
    () => new BookingCommands({ tenant, location, user: userId }, storage()),
  );
  const [busy, setBusy] = useState(false);
  const [editor, setEditor] = useState<Editor | null>(null);
  useEffect(() => {
    if (
      editor &&
      ((['record', 'appointment'].includes(editor.kind) && !canBook) ||
        (!['record', 'appointment'].includes(editor.kind) && !canManage))
    )
      setEditor(null);
  }, [canBook, canManage, editor]);
  const [success, setSuccess] = useState('');
  const [pageError, setPageError] = useState<unknown>(null);
  const [, render] = useState(0);
  const [online, setOnline] = useState(navigator.onLine);
  useEffect(() => {
    const changed = () => setOnline(navigator.onLine);
    window.addEventListener('online', changed);
    window.addEventListener('offline', changed);
    return () => {
      window.removeEventListener('online', changed);
      window.removeEventListener('offline', changed);
    };
  }, []);
  const disabled =
    busy || !online || Boolean(journal.storageError) || !canManage;
  const bookingDisabled =
    busy ||
    !online ||
    Boolean(journal.storageError) ||
    !canBook ||
    Boolean(journal.pending);
  async function run(command?: BookingCommand): Promise<BookingReceipt> {
    if (
      !canBook ||
      (!canManage &&
        !isAppointmentAction(command ?? journal.pending?.command ?? {}))
    )
      throw new ApiError(403, 'forbidden');
    if (!navigator.onLine) throw new ApiError(0, 'offline');
    setBusy(true);
    setPageError(null);
    setSuccess('');
    try {
      const receipt = await journal.run(command);
      if (receipt.resource.kind === 'batch') {
        setChosen(receipt.resource.item.specialist_id);
        if (receipt.resource.item.dates[0])
          setDate(receipt.resource.item.dates[0]);
      }
      if (
        receipt.resource.kind === 'offer' ||
        receipt.resource.kind === 'appointment'
      ) {
        const item = receipt.resource.item;
        setChosen(item.specialist_id);
        if (item.date) setDate(item.date);
        if (
          receipt.resource.kind === 'offer' &&
          receipt.resource.item.state === 'withdrawn'
        )
          setWithdrawn(true);
      } else if (
        receipt.resource.kind === 'specialist' &&
        command?.type === 'save_specialist' &&
        command.expected_version === 0
      )
        setChosen(receipt.resource.item.id);
      setSuccess(
        receipt.resource.kind === 'batch'
          ? `Добавлено талонов: ${receipt.resource.item.offer_ids.length}. Клиентские записи не копировались.`
          : receipt.resource.kind === 'appointment'
            ? receipt.resource.item.status === 'cancelled'
              ? 'Запись отменена. Место освобождено.'
              : 'Запись клиента сохранена.'
            : receipt.resource.kind === 'offer'
              ? 'Талон сохранён в расписании.'
              : receipt.resource.kind === 'preset'
                ? 'Заготовка сохранена. Ранее созданные талоны не изменились.'
                : 'Профиль специалиста сохранён.',
      );
      return receipt;
    } finally {
      try {
        await Promise.all([
          client.invalidateQueries({ queryKey: base }),
          client.invalidateQueries({ queryKey: keys.locations(tenant) }),
        ]);
      } finally {
        setBusy(false);
        render((n) => n + 1);
      }
    }
  }
  function open(value: Editor) {
    setSuccess('');
    setPageError(null);
    setEditor(value);
  }
  function openOffer(item: Offer | null, copy = false) {
    if (selected) open({ kind: 'offer', item, copy, specialist: selected });
  }
  const actionsDisabled = disabled || Boolean(journal.pending);
  function list(values: Offer[]) {
    return (
      <div className="booking-offer-list">
        {values.map((item) => {
          const people = clients.get(item.id) ?? [];
          const label = `${item.title || 'Приём'} ${timeLabel(item)}`;
          return (
            <article
              className={`booking-offer booking-offer-${item.state}${people.length ? ' is-booked' : ''}`}
              key={item.id}
            >
              <div className="booking-offer-time">
                <Icon name={item.mode === 'by_agreement' ? 'users' : 'clock'} />
                <strong>{timeLabel(item)}</strong>
                {item.mode === 'by_agreement' && (
                  <span>
                    {item.date
                      ? 'На выбранную дату'
                      : 'Дата тоже по согласованию'}
                  </span>
                )}
              </div>
              <div className="booking-offer-info">
                <div className="booking-offer-title">
                  <h3>{item.title || 'Приём'}</h3>
                  {item.state !== 'ready' && (
                    <span className={`booking-state state-${item.state}`}>
                      {offerStateLabels[item.state]}
                    </span>
                  )}
                </div>
                {(item.mode === 'by_agreement' || item.time_zone !== zone) && (
                  <p>
                    {item.mode === 'by_agreement'
                      ? `Лимит мест: ${item.places}`
                      : `Время ${item.time_zone}`}
                  </p>
                )}
                {people.length > 0 && (
                  <ul className="booking-offer-clients">
                    {people.map((a) => (
                      <li key={a.id}>
                        <button
                          type="button"
                          className="booking-client"
                          disabled={bookingDisabled}
                          onClick={() => open({ kind: 'appointment', item: a })}
                          aria-label={`Открыть запись ${a.client_name} ${timeLabel(a)}`}
                        >
                          <span className="booking-avatar" aria-hidden="true">
                            {[...a.client_name][0]?.toUpperCase()}
                          </span>
                          <span className="booking-client-copy">
                            <strong>{a.client_name}</strong>
                            <small>
                              {a.phone || 'Телефон не указан'}
                              {a.source === 'public' ? ' · онлайн-запись' : ''}
                            </small>
                          </span>
                          <Icon name="right" />
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
                {canBook && (!people.length || item.places > 1) && (
                  <p
                    className={`appointment-occupancy${canRecord(item, selected?.enabled ?? false) ? ' is-free' : ''}`}
                  >
                    {occupancyLabel(item)}
                  </p>
                )}
              </div>
              {(canBook || canManage) && (
                <div className="booking-offer-actions">
                  {canBook && canRecord(item, selected?.enabled ?? false) && (
                    <button
                      type="button"
                      className="button primary appointment-book"
                      disabled={bookingDisabled}
                      onClick={() => {
                        if (selected)
                          open({
                            kind: 'record',
                            offer: item,
                            specialist: selected,
                          });
                      }}
                      aria-label={`Записать клиента: ${label}`}
                    >
                      <Icon name="plus" />
                      Записать
                    </button>
                  )}
                  {canManage && (
                    <div className="booking-offer-tools">
                      <button
                        className="text-button"
                        type="button"
                        disabled={actionsDisabled}
                        onClick={() => openOffer(item)}
                        aria-label={`Изменить талон ${label}`}
                      >
                        Изменить
                      </button>
                      <button
                        className="text-button"
                        type="button"
                        disabled={actionsDisabled}
                        onClick={() => openOffer(item, true)}
                        aria-label={`Создать похожий талон ${label}`}
                      >
                        Похожий
                      </button>
                    </div>
                  )}
                </div>
              )}
            </article>
          );
        })}
      </div>
    );
  }
  const specialistPicker = specialists.length > 1 && (
    <div className="booking-specialists" role="group" aria-label="Выбор специалиста">
      {specialists.map((item) => (
        <button
          type="button"
          key={item.id}
          aria-pressed={item.id === selected?.id}
          onClick={() => setChosen(item.id)}
        >
          <span className="booking-avatar" aria-hidden="true">
            {[...item.name][0]}
          </span>
          <span>
            {item.name}
            {!item.enabled && <small>Выключен</small>}
          </span>
        </button>
      ))}
      {profiles.hasNextPage && (
        <button
          className="text-button"
          type="button"
          disabled={profiles.isFetching}
          onClick={() => {
            void profiles.fetchNextPage();
          }}
        >
          Ещё специалисты
        </button>
      )}
    </div>
  );
  const catalog = (
    <div className="booking-settings-grid">
      <section className="panel booking-settings-card" aria-labelledby="catalog-specialists">
        <div className="section-line">
          <div>
            <h2 id="catalog-specialists">Специалисты</h2>
            <p className="booking-settings-hint">
              Кто ведёт приём в этой точке. Имя и специализацию видят клиенты
              онлайн-записи.
            </p>
          </div>
          {canManage && (
            <button
              type="button"
              className="button secondary"
              disabled={actionsDisabled}
              onClick={() => open({ kind: 'specialist', item: null })}
            >
              <Icon name="plus" />
              Добавить
            </button>
          )}
        </div>
        {!specialists.length && (
          <p className="booking-settings-empty">Специалистов пока нет.</p>
        )}
        {specialists.map((item) => (
          <div className="booking-setting-row" key={item.id}>
            <span className="booking-avatar" aria-hidden="true">
              {[...item.name][0]}
            </span>
            <div>
              <strong>{item.name}</strong>
              <p>
                {item.description || 'Специализация не указана'}
                {!item.enabled ? ' · выключен' : ''}
              </p>
            </div>
            {canManage && (
              <button
                type="button"
                className="text-button"
                disabled={actionsDisabled}
                onClick={() => open({ kind: 'specialist', item })}
                aria-label={`Изменить специалиста ${item.name}`}
              >
                Изменить
              </button>
            )}
          </div>
        ))}
        {profiles.hasNextPage && (
          <button
            className="text-button"
            type="button"
            disabled={profiles.isFetching}
            onClick={() => {
              void profiles.fetchNextPage();
            }}
          >
            Ещё специалисты
          </button>
        )}
      </section>
      <section className="panel booking-settings-card" aria-labelledby="catalog-presets">
        <div className="section-line">
          <div>
            <h2 id="catalog-presets">Заготовки приёмов</h2>
            <p className="booking-settings-hint">
              Необязательные названия и длительности для быстрого заполнения.
              Ручной ввод доступен всегда.
            </p>
          </div>
          {canManage && (
            <button
              type="button"
              className="button secondary"
              disabled={actionsDisabled}
              onClick={() => open({ kind: 'preset', item: null })}
            >
              <Icon name="plus" />
              Добавить
            </button>
          )}
        </div>
        {presets.isPending ? (
          <Loading label="Загружаем заготовки…" />
        ) : presets.isError ? (
          <ErrorNotice
            error={presets.error}
            retry={() => {
              void presets.refetch();
            }}
          />
        ) : templates.length ? (
          templates.map((item) => (
            <div className="booking-setting-row" key={item.id}>
              <span className="booking-setting-icon" aria-hidden="true">
                <Icon name="clock" />
              </span>
              <div>
                <strong>{item.name}</strong>
                <p>
                  {item.duration_minutes
                    ? `${item.duration_minutes} мин`
                    : 'Без заданной длительности'}
                  {!item.enabled ? ' · выключена' : ''}
                </p>
              </div>
              {canManage && (
                <button
                  type="button"
                  className="text-button"
                  disabled={actionsDisabled}
                  onClick={() => open({ kind: 'preset', item })}
                  aria-label={`Изменить заготовку ${item.name}`}
                >
                  Изменить
                </button>
              )}
            </div>
          ))
        ) : (
          <p className="booking-settings-empty">Заготовок пока нет.</p>
        )}
        {presets.hasNextPage && (
          <button
            className="text-button"
            type="button"
            disabled={presets.isFetching}
            onClick={() => {
              void presets.fetchNextPage();
            }}
          >
            Ещё заготовки
          </button>
        )}
      </section>
    </div>
  );
  return (
    <>
      {!online && (
        <p className="conflict-notice" role="status">
          Нет подключения. Сохранение станет доступно после восстановления
          связи.
        </p>
      )}
      {journal.storageError !== null && (
        <p className="error-notice" role="alert">
          Не удалось прочитать или сохранить незавершённое действие. Проверьте
          хранилище браузера и перезагрузите страницу.
        </p>
      )}
      {journal.pending && !busy && (
        <div className="queue-pending" role="alert">
          <strong>Результат сохранения ещё не подтверждён</strong>
          <p>
            Повтор восстановит тот же запрос. Новое сохранение доступно после
            получения результата.
          </p>
          {canBook && (
            <button
              type="button"
              className="button secondary"
              disabled={busy || !online || Boolean(journal.storageError)}
              onClick={() => {
                void run().catch(setPageError);
              }}
            >
              Повторить прежний запрос
            </button>
          )}
        </div>
      )}
      {success && <SuccessNotice>{success}</SuccessNotice>}
      {pageError !== null && (
        <p className="error-notice" role="alert">
          {bookingError(pageError) ?? errorMessage(pageError)}
        </p>
      )}
      {profiles.isPending ? (
        <Loading label="Загружаем специалистов…" />
      ) : profiles.isError ? (
        <ErrorNotice
          error={profiles.error}
          retry={() => {
            void profiles.refetch();
          }}
        />
      ) : view === 'catalog' ? (
        catalog
      ) : !specialists.length ? (
        <section className="panel booking-onboarding">
          <span className="large-icon">
            <Icon name="users" />
          </span>
          <div>
            <p className="small-eyebrow">НАЧНЁМ СО СПЕЦИАЛИСТА</p>
            <h2>Кто принимает посетителей?</h2>
            <p>
              {canManage
                ? 'Добавьте имя и специализацию. Затем можно подготовить талоны — с точным временем или по договорённости.'
                : 'Управляющий ещё не добавил специалистов этой точки.'}
            </p>
            {canManage && (
              <button
                className="button primary"
                type="button"
                disabled={actionsDisabled}
                onClick={() => open({ kind: 'specialist', item: null })}
              >
                <Icon name="plus" />
                Добавить специалиста
              </button>
            )}
          </div>
        </section>
      ) : (
        <>
          {specialistPicker}
          <div className="booking-daybar">
            <div className="booking-day-picker">
              <button
                className="icon-button booking-day-step"
                type="button"
                aria-label="Предыдущий день"
                onClick={() => setDate(shiftDate(date, -1))}
              >
                <Icon name="left" />
              </button>
              <label className="booking-date-input">
                <span className="visually-hidden">Дата расписания</span>
                <input
                  id="booking-date"
                  type="date"
                  value={date}
                  onChange={(e) => {
                    if (e.target.value) setDate(e.target.value);
                  }}
                />
              </label>
              <button
                className="icon-button booking-day-step"
                type="button"
                aria-label="Следующий день"
                onClick={() => setDate(shiftDate(date, 1))}
              >
                <Icon name="right" />
              </button>
              {date !== today && (
                <button
                  className="button secondary booking-today"
                  type="button"
                  onClick={() => setDate(today)}
                >
                  Сегодня
                </button>
              )}
            </div>
            {canManage && (
              <div className="booking-schedule-actions">
                <button
                  className="button secondary"
                  type="button"
                  disabled={actionsDisabled || !selected}
                  onClick={() =>
                    selected && open({ kind: 'batch', specialist: selected })
                  }
                >
                  <Icon name="calendar" />
                  Заполнить на несколько дней
                </button>
                <button
                  className="button primary"
                  type="button"
                  disabled={actionsDisabled || !selected}
                  onClick={() => openOffer(null)}
                >
                  <Icon name="plus" />
                  Добавить талон
                </button>
              </div>
            )}
          </div>
          {selected && !selected.enabled && (
            <p className="conflict-notice" role="status">
              Специалист выключен. Расписание сохранено; новые талоны можно
              оставить в черновиках.
            </p>
          )}
          <div className="booking-day-head">
            <div>
              <h2>{dayTitle(date, today)}</h2>
              <p>
                {selected?.name}
                {offers.isSuccess && items.length > 0 && (
                  <>
                    {' · '}
                    {canBook
                      ? `приёмов: ${ready.length}${offers.hasNextPage ? '+' : ''}, записано: ${booked}, свободно: ${free}`
                      : `талонов: ${items.length}${offers.hasNextPage ? '+' : ''}`}
                  </>
                )}
              </p>
            </div>
            <div className="booking-day-filters" role="group" aria-label="Показать в расписании">
              <label className="checkbox-label">
                <input
                  type="checkbox"
                  checked={withdrawn}
                  onChange={(e) => setWithdrawn(e.target.checked)}
                />
                Снятые талоны
              </label>
              {canBook && (
                <label className="checkbox-label">
                  <input
                    type="checkbox"
                    checked={cancelled}
                    onChange={(e) => setCancelled(e.target.checked)}
                  />
                  Отменённые записи
                </label>
              )}
            </div>
          </div>
          {offers.isPending ? (
            <Loading label="Загружаем расписание…" />
          ) : offers.isError ? (
            <ErrorNotice
              error={offers.error}
              retry={() => {
                void offers.refetch();
              }}
            />
          ) : !items.length ? (
            <div className="panel booking-empty">
              <Icon name="calendar" />
              <h2>
                {date === today ? 'Сегодня приёмов нет' : 'На этот день приёмов нет'}
              </h2>
              {nextDay.data ? (
                <p>
                  Ближайший день с приёмами —{' '}
                  {dayTitle(nextDay.data.date, today).toLowerCase()} (приёмов:{' '}
                  {nextDay.data.count}
                  {canBook ? `, записано: ${nextDay.data.booked}` : ''}).
                </p>
              ) : (
                <p>
                  {canManage
                    ? 'Заполните расписание сразу на несколько дней или добавьте отдельный талон. Время окончания можно не указывать; доступны и талоны по договорённости.'
                    : 'На выбранный день пока нет талонов. Выберите другую дату или специалиста.'}
                </p>
              )}
              <div className="booking-empty-actions">
                {nextDay.data && (
                  <button
                    className="button primary"
                    type="button"
                    onClick={() => setDate(nextDay.data!.date)}
                  >
                    Перейти к этому дню
                    <Icon name="right" />
                  </button>
                )}
                {canManage && (
                  <>
                    <button
                      className={nextDay.data ? 'button secondary' : 'button primary'}
                      type="button"
                      disabled={actionsDisabled || !selected}
                      onClick={() =>
                        selected && open({ kind: 'batch', specialist: selected })
                      }
                    >
                      <Icon name="calendar" />
                      Заполнить на несколько дней
                    </button>
                    <button
                      className="button secondary"
                      type="button"
                      disabled={actionsDisabled || !selected}
                      onClick={() => openOffer(null)}
                    >
                      <Icon name="plus" />
                      Добавить один талон
                    </button>
                  </>
                )}
              </div>
            </div>
          ) : (
            <>
              {timed.length > 0 && (
                <section
                  className="booking-section"
                  aria-label="Назначенное время"
                >
                  {list(timed)}
                </section>
              )}
              {agreement.length > 0 && (
                <section
                  className="booking-section"
                  aria-labelledby="agreement-offers"
                >
                  <div className="section-line">
                    <h2 id="agreement-offers">По договорённости</h2>
                    <span className="subtle">Время согласуете отдельно</span>
                  </div>
                  {list(agreement)}
                </section>
              )}
            </>
          )}
          {offers.hasNextPage && (
            <div className="pagination">
              <button
                type="button"
                className="button secondary"
                disabled={offers.isFetching}
                onClick={() => {
                  void offers.fetchNextPage();
                }}
              >
                Загрузить ещё талоны
              </button>
            </div>
          )}
          {canBook && selected && (
            <AppointmentsPanel
              tenant={tenant}
              location={location}
              specialist={selected.id}
              date={date}
              placed={placed}
              cancelled={cancelled}
              disabled={bookingDisabled}
              open={(item) => open({ kind: 'appointment', item })}
            />
          )}
        </>
      )}
      {editor &&
        (editor.kind === 'batch' ? (
          <BookingBatchEditor
            tenant={tenant}
            location={location}
            specialist={
              specialists.find((s) => s.id === editor.specialist.id) ??
              editor.specialist
            }
            zone={zone}
            date={date}
            presets={templates}
            morePresets={
              presets.hasNextPage
                ? () => void presets.fetchNextPage()
                : undefined
            }
            createPreset={() =>
              open({ kind: 'preset', item: null, back: editor.specialist })
            }
            sourceEmpty={!items.length}
            busy={busy}
            journal={journal}
            run={run}
            close={() => setEditor(null)}
          />
        ) : editor.kind === 'record' ? (
          <RecordAppointmentEditor
            key={`record:${editor.offer.id}`}
            offer={editor.offer}
            current={items.find((i) => i.id === editor.offer.id)}
            specialist={
              specialists.find((s) => s.id === editor.specialist.id) ??
              editor.specialist
            }
            busy={busy}
            journal={journal}
            run={run}
            close={() => setEditor(null)}
          />
        ) : editor.kind === 'appointment' ? (
          <AppointmentEditor
            key={`appointment:${editor.item.id}`}
            tenant={tenant}
            location={location}
            initial={editor.item}
            busy={busy}
            journal={journal}
            run={run}
            close={() => setEditor(null)}
          />
        ) : editor.kind === 'offer' ? (
          <OfferEditor
            tenant={tenant}
            location={location}
            key={`offer:${editor.item?.id ?? 'new'}:${Boolean(editor.copy)}`}
            initial={editor.item}
            copy={editor.copy}
            current={items.find((i) => i.id === editor.item?.id)}
            specialist={
              specialists.find((s) => s.id === editor.specialist.id) ??
              editor.specialist
            }
            date={date}
            zone={zone}
            presets={templates}
            morePresets={
              presets.hasNextPage
                ? () => {
                    void presets.fetchNextPage();
                  }
                : undefined
            }
            busy={busy}
            journal={journal}
            run={run}
            close={() => setEditor(null)}
          />
        ) : (
          <CatalogEditor
            key={`${editor.kind}:${editor.item?.id ?? 'new'}`}
            kind={editor.kind}
            initial={editor.item}
            current={
              editor.kind === 'specialist'
                ? specialists.find((i) => i.id === editor.item?.id)
                : templates.find((i) => i.id === editor.item?.id)
            }
            busy={busy}
            journal={journal}
            run={run}
            close={() =>
              setEditor(
                editor.kind === 'preset' && editor.back
                  ? { kind: 'batch', specialist: editor.back }
                  : null,
              )
            }
          />
        ))}
    </>
  );
}
