import { Icon, Modal } from '../components';
import { ClientFields, Failure, ScheduleSummary } from './editorShared';
import type { RecordAppointmentEditorProps } from './useRecordAppointment';
import { useRecordAppointment } from './useRecordAppointment';

export function RecordAppointmentEditor(props: RecordAppointmentEditorProps) {
  const { offer, current, specialist, busy, journal, close } = props;
  const {
    client,
    setClient,
    setVersion,
    error,
    setError,
    invalid,
    setInvalid,
    changed,
    unavailable,
    save,
    submit,
  } = useRecordAppointment(props);
  return (
    <Modal
      icon="users"
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
