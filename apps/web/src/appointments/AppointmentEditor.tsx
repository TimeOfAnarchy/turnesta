import { ApiError } from '../api';
import { timeLabel } from '../bookings';
import { ClientExportDialog } from '../ClientExportDialog';
import { ErrorNotice, Loading, Modal } from '../components';
import { ClientFields, Failure, ScheduleSummary, dateLabel } from './editorShared';
import type { AppointmentEditorProps } from './useAppointmentCommand';
import { useAppointmentCommand } from './useAppointmentCommand';

export function AppointmentEditor(props: AppointmentEditorProps) {
  const { tenant, location, busy, journal, close } = props;
  const {
    item,
    client,
    setClient,
    mode,
    backButton,
    date,
    setDate,
    target,
    setTarget,
    error,
    setError,
    invalid,
    setInvalid,
    loading,
    offers,
    options,
    staleTarget,
    blocked,
    canErase,
    canExport,
    changeMode,
    refresh,
    save,
    submit,
  } = useAppointmentCommand(props);
  if (mode === 'export') {
    if (!canExport) return <Modal title="Выгрузка недоступна" description="Проверьте права доступа и данные записи." close={close}>
      <button type="button" className="button secondary" onClick={() => changeMode('view')}>Назад к записи</button>
    </Modal>;
    return <ClientExportDialog tenant={tenant} location={location} item={item}
      back={() => changeMode('view')} close={close}/>;
  }
  return (
    <Modal
      icon="users"
      title={
        mode === 'anonymize'
          ? 'Удалить данные клиента?'
          : mode === 'cancel'
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
                  {!item.anonymized_at && <button
                    type="button"
                    className="button secondary"
                    onClick={() => changeMode('edit')}
                  >
                    Изменить данные
                  </button>}
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
              {canExport && (item.phone || item.email
                ? <button type="button" className="text-button" onClick={() => changeMode('export')}>Выгрузить записи клиента</button>
                : <p className="field-hint">Для выгрузки записей нужен телефон или email клиента.</p>)}
              {canErase && (
                <button type="button" className="text-button danger" onClick={() => changeMode('anonymize')}>
                  Удалить данные клиента
                </button>
              )}
            </>
          )}
          {mode === 'anonymize' && (
            <div className="conflict-notice">
              <p>Имя, контакты и ответы анкеты будут удалены без возможности восстановления. Факт и время записи сохранятся.</p>
              <p>В резервных копиях и незавершённых запросах на устройствах прежние данные могут сохраняться.</p>
            </div>
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
              ref={backButton}
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
              className={`button ${mode === 'cancel' || mode === 'anonymize' ? 'appointment-cancel' : 'primary'}`}
              disabled={
                blocked ||
                (item.status !== 'confirmed' && mode !== 'anonymize') ||
                (mode === 'anonymize' && !canErase) ||
                (mode === 'move' && (!target || staleTarget))
              }
            >
              {busy
                ? 'Сохраняем…'
                : mode === 'anonymize'
                  ? 'Да, удалить данные'
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
