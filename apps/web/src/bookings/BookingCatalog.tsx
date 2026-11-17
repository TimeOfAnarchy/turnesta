import { ErrorNotice, Icon, Loading } from '../components';
import type { BookingDay } from './useBookingDay';

export function BookingCatalog({
  canManage,
  profiles,
  presets,
  specialists,
  templates,
  open,
  actionsDisabled,
}: Pick<
  BookingDay,
  | 'canManage'
  | 'profiles'
  | 'presets'
  | 'specialists'
  | 'templates'
  | 'open'
  | 'actionsDisabled'
>) {
  return (
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
}
