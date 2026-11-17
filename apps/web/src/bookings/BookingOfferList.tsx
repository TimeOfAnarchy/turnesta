import { canRecord, occupancyLabel } from '../appointments';
import type { Offer } from '../bookings';
import { offerStateLabels, timeLabel } from '../bookings';
import { Icon } from '../components';
import type { BookingDay } from './useBookingDay';
export function BookingOfferList({
  values,
  zone,
  canManage,
  canBook,
  selected,
  clients,
  bookingDisabled,
  open,
  openOffer,
  openAppointment,
  actionsDisabled,
}: Pick<
  BookingDay,
  | 'zone'
  | 'canManage'
  | 'canBook'
  | 'selected'
  | 'clients'
  | 'bookingDisabled'
  | 'open'
  | 'openOffer'
  | 'openAppointment'
  | 'actionsDisabled'
> & { values: Offer[] }) {
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
                        onClick={() => openAppointment(a)}
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
