import { useEffect } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { useWorkspace } from './App';
import { BookingsHelp } from './BookingsHelp';
import { PublicBookingSettings } from './PublicBookingSettings';
import { BookingSchedule as Schedule } from './bookings/BookingSchedule';
import { distinct } from './bookings/collections';
import { ErrorNotice, Icon, Loading } from './components';
import { useLocations } from './queries';
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
  const { fetchNextPage: fetchMoreLocations } = locations;
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
      void fetchMoreLocations();
  }, [
    location,
    point,
    locations.hasNextPage,
    locations.isFetching,
    locations.isError,
    fetchMoreLocations,
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
