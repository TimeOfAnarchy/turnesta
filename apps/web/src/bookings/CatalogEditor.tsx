import type { Preset, Specialist } from '../bookings';
import { Modal } from '../components';
import { FormFailure } from './editorShared';
import type { CatalogEditorProps } from './useCatalogEditor';
import { useCatalogEditor } from './useCatalogEditor';

export function CatalogEditor(props: CatalogEditorProps) {
  const { kind, initial, current, busy, journal, close } = props;
  const {
    name,
    setName,
    description,
    setDescription,
    minutes,
    setMinutes,
    enabled,
    setEnabled,
    setVersion,
    error,
    setError,
    invalid,
    conflict,
    save,
    submit,
  } = useCatalogEditor(props);
  return (
    <Modal
      icon={kind === 'specialist' ? 'users' : 'clock'}
      title={
        kind === 'specialist'
          ? initial
            ? 'Изменить специалиста'
            : 'Новый специалист'
          : initial
            ? 'Изменить заготовку'
            : 'Новая заготовка'
      }
      description={
        kind === 'specialist'
          ? 'Профиль специалиста относится к этой точке. Выключение сохраняет ранее подготовленное расписание.'
          : 'Заготовка подставляет значения в новый талон. Изменение заготовки не меняет расписание.'
      }
      busy={busy}
      close={close}
    >
      <form className="booking-form" onSubmit={submit} noValidate>
        <fieldset disabled={busy || Boolean(journal.pending)}>
          <div className="form-field">
            <label htmlFor="booking-name">
              {kind === 'specialist' ? 'Имя специалиста' : 'Название заготовки'}
            </label>
            <input
              id="booking-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              autoComplete="off"
            />
          </div>
          {kind === 'specialist' ? (
            <div className="form-field">
              <label htmlFor="booking-description">
                Специализация или описание{' '}
                <span className="subtle">· необязательно</span>
              </label>
              <input
                id="booking-description"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
              />
            </div>
          ) : (
            <div className="form-field">
              <label htmlFor="booking-minutes">
                Длительность, минут{' '}
                <span className="subtle">· необязательно</span>
              </label>
              <input
                id="booking-minutes"
                type="number"
                min="1"
                max="1440"
                value={minutes}
                onChange={(e) => setMinutes(e.target.value)}
                aria-describedby="booking-minutes-hint"
              />
              <p className="field-hint" id="booking-minutes-hint">
                Нужна, чтобы заполнять расписание серией на несколько дней.
              </p>
            </div>
          )}
          <label className="service-enabled">
            <input
              type="checkbox"
              checked={enabled}
              onChange={(e) => setEnabled(e.target.checked)}
            />
            {kind === 'specialist'
              ? 'Специалист принимает посетителей'
              : 'Доступна для выбора'}
          </label>
        </fieldset>
        {invalid && (
          <p className="field-error" role="alert">
            {invalid}
          </p>
        )}
        {conflict && (
          <div className="conflict-notice" role="alert">
            <p>Данные уже изменились. Ваш ввод сохранён в форме.</p>
            <button
              className="text-button"
              type="button"
              disabled={busy || Boolean(journal.pending)}
              onClick={() => {
                setName(current!.name);
                setEnabled(current!.enabled);
                setVersion(current!.version);
                if ('description' in current!)
                  setDescription((current as Specialist).description);
                if ('duration_minutes' in current!)
                  setMinutes(
                    (current as Preset).duration_minutes === null
                      ? ''
                      : String((current as Preset).duration_minutes),
                  );
                setError(null);
              }}
            >
              Загрузить сохранённую версию
            </button>
          </div>
        )}
        <FormFailure
          error={error}
          pending={Boolean(journal.pending)}
          busy={busy}
          retry={() => {
            void save();
          }}
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
            type="submit"
            disabled={busy || Boolean(journal.pending) || conflict}
          >
            {busy ? 'Сохраняем…' : 'Сохранить'}
          </button>
        </div>
      </form>
    </Modal>
  );
}
