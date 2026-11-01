import { useEffect, useState } from 'react';
import { useInfiniteQuery, useQueryClient } from '@tanstack/react-query';
import { Link, useSearchParams } from 'react-router-dom';
import { useWorkspace } from './App';
import { ApiError, errorMessage } from './api';
import { ErrorNotice, Icon, Loading, SuccessNotice } from './components';
import { keys, useLocations } from './queries';
import { OfferEditor, CatalogEditor } from './BookingEditors';
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
export function BookingsPage() {
  const { organization } = useWorkspace();
  const locations = useLocations(organization.id);
  const [params, setParams] = useSearchParams();
  const items = distinct(locations.data?.pages.flatMap((p) => p.items) ?? []);
  const location = params.get('location') ?? items[0]?.id ?? '';
  const point = items.find((i) => i.id === location);
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
  return (
    <>
      <div className="page-heading">
        <div>
          <p className="eyebrow">ОРГАНИЗАЦИЯ / ЗАПИСЬ</p>
          <h1>
            Расписание приёмов<span className="heading-dot">.</span>
          </h1>
          <p className="page-intro">
            Подготовьте время для посетителей — вручную или с помощью заготовок.
          </p>
        </div>
        <span className="booking-heading-icon" aria-hidden="true">
          <Icon name="calendar" />
        </span>
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
          <Icon name="locations" />
          <h2>Сначала добавьте точку</h2>
          <p>Специалисты и расписание относятся к месту приёма.</p>
          <Link
            className="button primary"
            to={`/organizations/${organization.id}/locations`}
          >
            К точкам
          </Link>
        </div>
      ) : (
        <>
          <div className="booking-location">
            <div className="form-field">
              <label htmlFor="booking-location">Точка приёма</label>
              <select
                id="booking-location"
                value={location}
                onChange={(e) => setParams({ location: e.target.value })}
              >
                {!point && (
                  <option value={location}>Загружаем выбранную точку…</option>
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
          </div>
          {point && (
            <Schedule
              key={`${organization.id}:${location}`}
              tenant={organization.id}
              location={location}
              zone={point.time_zone}
            />
          )}
        </>
      )}
    </>
  );
}
type Editor =
  | { kind: 'specialist'; item: Specialist | null }
  | { kind: 'preset'; item: Preset | null }
  | {
      kind: 'offer';
      item: Offer | null;
      copy?: boolean;
      specialist: Specialist;
    };
function Schedule({
  tenant,
  location,
  zone,
}: {
  tenant: string;
  location: string;
  zone: string;
}) {
  const { canManage, userId } = useWorkspace();
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
  const [date, setDate] = useState(() => todayAt(zone));
  const [withdrawn, setWithdrawn] = useState(false);
  const offers = useInfiniteQuery({
    queryKey: [...base, 'offers', selected?.id, date, withdrawn],
    initialPageParam: null as string | null,
    enabled: Boolean(selected),
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
  const items = distinct(offers.data?.pages.flatMap((p) => p.items) ?? []);
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
  const [journal] = useState(
    () => new BookingCommands({ tenant, location, user: userId }, storage()),
  );
  const [busy, setBusy] = useState(false);
  const [editor, setEditor] = useState<Editor | null>(null);
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
  async function run(command?: BookingCommand): Promise<BookingReceipt> {
    if (!canManage) throw new ApiError(403, 'forbidden');
    if (!navigator.onLine) throw new ApiError(0, 'offline');
    setBusy(true);
    setPageError(null);
    setSuccess('');
    try {
      const receipt = await journal.run(command);
      if (receipt.resource.kind === 'offer') {
        const item = receipt.resource.item;
        setChosen(item.specialist_id);
        if (item.date) setDate(item.date);
        if (item.state === 'withdrawn') setWithdrawn(true);
      } else if (
        receipt.resource.kind === 'specialist' &&
        command?.expected_version === 0
      )
        setChosen(receipt.resource.item.id);
      setSuccess(
        receipt.resource.kind === 'offer'
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
        {values.map((item) => (
          <article
            className={`booking-offer booking-offer-${item.state}`}
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
              <h3>{item.title || 'Приём'}</h3>
              <p>
                {item.mode === 'by_agreement'
                  ? `Лимит мест: ${item.places}`
                  : item.time_zone === zone
                    ? selected?.name
                    : `${selected?.name} · ${item.time_zone}`}
              </p>
            </div>
            <span className={`booking-state state-${item.state}`}>
              {offerStateLabels[item.state]}
            </span>
            {canManage && (
              <div className="booking-offer-actions">
                <button
                  className="text-button"
                  type="button"
                  disabled={actionsDisabled}
                  onClick={() => openOffer(item)}
                  aria-label={`Изменить талон ${item.title || 'Приём'} ${timeLabel(item)}`}
                >
                  Изменить
                </button>
                <button
                  className="text-button"
                  type="button"
                  disabled={actionsDisabled}
                  onClick={() => openOffer(item, true)}
                  aria-label={`Создать похожий талон ${item.title || 'Приём'} ${timeLabel(item)}`}
                >
                  Создать похожий
                </button>
              </div>
            )}
          </article>
        ))}
      </div>
    );
  }
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
          {canManage && (
            <button
              type="button"
              className="button secondary"
              disabled={disabled}
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
          <div className="booking-specialist-line">
            <span className="small-eyebrow">СПЕЦИАЛИСТ</span>
            <div
              className="booking-specialists"
              role="group"
              aria-label="Выбор специалиста"
            >
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
          </div>
          <div className="booking-daybar">
            <div className="booking-day-picker">
              <button
                className="icon-button"
                type="button"
                aria-label="Предыдущий день"
                onClick={() => setDate(shiftDate(date, -1))}
              >
                <span aria-hidden="true">←</span>
              </button>
              <div className="form-field">
                <label htmlFor="booking-date">Дата расписания</label>
                <input
                  id="booking-date"
                  type="date"
                  value={date}
                  onChange={(e) => {
                    if (e.target.value) setDate(e.target.value);
                  }}
                />
              </div>
              <button
                className="icon-button"
                type="button"
                aria-label="Следующий день"
                onClick={() => setDate(shiftDate(date, 1))}
              >
                <span aria-hidden="true">→</span>
              </button>
              <button
                className="text-button"
                type="button"
                onClick={() => setDate(todayAt(zone))}
              >
                Сегодня
              </button>
            </div>
            {canManage && (
              <button
                className="button primary"
                type="button"
                disabled={actionsDisabled || !selected}
                onClick={() => openOffer(null)}
              >
                <Icon name="plus" />
                Добавить талон
              </button>
            )}
          </div>
          {selected && !selected.enabled && (
            <p className="conflict-notice" role="status">
              Специалист выключен. Расписание сохранено; новые талоны можно
              оставить в черновиках.
            </p>
          )}
          <div className="booking-day-caption">
            <span>
              {new Intl.DateTimeFormat('ru-RU', {
                dateStyle: 'full',
                timeZone: 'UTC',
              }).format(new Date(`${date}T12:00:00Z`))}
            </span>
            <label>
              <input
                type="checkbox"
                checked={withdrawn}
                onChange={(e) => setWithdrawn(e.target.checked)}
              />
              Показать снятые
            </label>
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
              <h2>День ещё свободен</h2>
              <p>
                {canManage
                  ? 'Добавьте талон вручную. Время окончания можно не указывать; доступны и талоны по договорённости.'
                  : 'На выбранный день пока нет талонов. Выберите другую дату или специалиста.'}
              </p>
            </div>
          ) : (
            <>
              {timed.length > 0 && (
                <section
                  className="booking-section"
                  aria-labelledby="timed-offers"
                >
                  <div className="section-line">
                    <h2 id="timed-offers">Назначенное время</h2>
                    <span className="subtle">
                      Талонов: {timed.length}
                      {offers.hasNextPage ? '+' : ''}
                    </span>
                  </div>
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
        </>
      )}
      <details className="booking-settings panel">
        <summary>
          <Icon name="settings" />
          <span>
            <strong>Специалисты и заготовки</strong>
            <span>Управление приёмом и быстрым заполнением</span>
          </span>
          <Icon name="chevron" />
        </summary>
        <div className="booking-settings-grid">
          <section>
            <div className="section-line">
              <h2>Специалисты</h2>
              {canManage && (
                <button
                  type="button"
                  className="text-button"
                  disabled={actionsDisabled}
                  onClick={() => open({ kind: 'specialist', item: null })}
                >
                  Добавить
                </button>
              )}
            </div>
            {specialists.map((item) => (
              <div className="booking-setting-row" key={item.id}>
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
          <section>
            <div className="section-line">
              <h2>Заготовки приёмов</h2>
              {canManage && (
                <button
                  type="button"
                  className="text-button"
                  disabled={actionsDisabled}
                  onClick={() => open({ kind: 'preset', item: null })}
                >
                  Добавить заготовку
                </button>
              )}
            </div>
            <p className="booking-settings-hint">
              Необязательные названия и длительности для быстрого ввода. Ручное
              заполнение всегда доступно.
            </p>
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
              <p className="subtle">Заготовок пока нет.</p>
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
      </details>
      {editor &&
        (editor.kind === 'offer' ? (
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
            close={() => setEditor(null)}
          />
        ))}
    </>
  );
}
