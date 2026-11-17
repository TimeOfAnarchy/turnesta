import { AppointmentsPanel } from '../AppointmentEditors';
import { errorMessage } from '../api';
import { bookingError, shiftDate } from '../bookings';
import { ErrorNotice, Icon, Loading, SuccessNotice } from '../components';
import { BookingCatalog } from './BookingCatalog';
import { BookingDialogs } from './BookingDialogs';
import { BookingOfferList } from './BookingOfferList';
import type { BookingScheduleProps } from './useBookingDay';
import { useBookingDay } from './useBookingDay';
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
export function BookingSchedule(props: BookingScheduleProps) {
  const day = useBookingDay(props);
  const {
    view,
    canManage,
    canBook,
    profiles,
    specialists,
    setChosen,
    selected,
    today,
    date,
    setDate,
    withdrawn,
    setWithdrawn,
    cancelled,
    setCancelled,
    offers,
    appointments,
    history,
    items,
    nextDay,
    timed,
    agreement,
    placed,
    ready,
    booked,
    free,
    journal,
    busy,
    success,
    pageError,
    setPageError,
    online,
    bookingDisabled,
    run,
    open,
    openOffer,
    openAppointment,
    actionsDisabled,
  } = day;
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
          {canBook && (canManage || journal.pending.command.type !== 'anonymize_appointment') ? (
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
          ) : journal.pending.command.type === 'anonymize_appointment' ? (
            <p>Для восстановления этого запроса нужны права владельца или управляющего.</p>
          ) : null}
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
        <BookingCatalog {...day} />
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
                  <BookingOfferList {...day} values={timed} />
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
                  <BookingOfferList {...day} values={agreement} />
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
              query={cancelled ? history : appointments}
              placed={placed}
              cancelled={cancelled}
              disabled={bookingDisabled}
              open={openAppointment}
            />
          )}
        </>
      )}
      <BookingDialogs {...day} />
    </>
  );
}
