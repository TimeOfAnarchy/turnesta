import { errorMessage } from '../api';
import type { OfferState } from '../bookings';
import { bookingError, timeLabel } from '../bookings';
import { Icon, Modal, TimeInput } from '../components';
import { FormFailure } from './editorShared';
import type { OfferEditorProps } from './useOfferEditor';
import { offerDraft, useOfferEditor } from './useOfferEditor';

export function OfferEditor(props: OfferEditorProps) {
  const {
    initial,
    copy,
    current,
    specialist,
    date,
    presets,
    morePresets,
    busy,
    journal,
    close,
  } = props;
  const {
    draft,
    setDraft,
    setVersion,
    presetId,
    setAutoDuration,
    notice,
    setNotice,
    error,
    setError,
    invalid,
    conflict,
    timezone,
    occupied,
    calculationPending,
    calculationError,
    update,
    choosePreset,
    save,
    submit,
  } = useOfferEditor(props);
  return (
    <Modal
      icon="clock"
      title={
        initial && !copy
          ? 'Изменить талон'
          : copy
            ? 'Похожий талон'
            : 'Новый талон'
      }
      description={`${specialist.name}. Время указывается по часовому поясу точки (${timezone}).`}
      busy={busy}
      close={close}
    >
      <form className="booking-form" onSubmit={submit} noValidate>
        {occupied > 0 && (
          <p className="conflict-notice">
            На талон записаны клиенты: {occupied}. Время можно изменить после
            переноса или отмены их записей. Можно изменить название, увеличить
            лимит или закрыть талон для новых записей.
          </p>
        )}
        <fieldset disabled={busy || Boolean(journal.pending)}>
          <div
            className="booking-mode"
            role="group"
            aria-label="Назначение времени"
          >
            <button
              type="button"
              aria-pressed={!draft.agreement}
              disabled={occupied > 0}
              onClick={() => update({ agreement: false })}
            >
              <Icon name="clock" />
              Назначить время
            </button>
            <button
              type="button"
              aria-pressed={draft.agreement}
              disabled={occupied > 0}
              onClick={() => update({ agreement: true })}
            >
              <Icon name="users" />
              По договорённости
            </button>
          </div>
          {(presets.some((p) => p.enabled) || morePresets) && (
            <details className="booking-preset-picker">
              <summary>
                Заполнить из заготовки{' '}
                <span className="subtle">· необязательно</span>
              </summary>
              <div className="form-field">
                <label htmlFor="offer-preset">Заготовка приёма</label>
                <select
                  id="offer-preset"
                  disabled={occupied > 0}
                  value={presetId}
                  onChange={(e) => choosePreset(e.target.value)}
                >
                  <option value="">Ручное заполнение</option>
                  {presets
                    .filter((p) => p.enabled)
                    .map((p) => (
                      <option key={p.id} value={p.id}>
                        {p.name}
                        {p.duration_minutes ? ` · ${p.duration_minutes} мин` : ''}
                      </option>
                    ))}
                </select>
              </div>
              {morePresets && (
                <button
                  className="text-button"
                  type="button"
                  onClick={morePresets}
                >
                  Загрузить ещё заготовки
                </button>
              )}
            </details>
          )}
          <div className="form-field">
            <label htmlFor="offer-title">
              Название приёма <span className="subtle">· необязательно</span>
            </label>
            <input
              id="offer-title"
              value={draft.title}
              onChange={(e) => update({ title: e.target.value })}
              autoComplete="off"
            />
          </div>
          {draft.agreement ? (
            <>
              <div className="booking-form-grid">
                <div className="form-field">
                  <label htmlFor="offer-agreement-date">
                    Дата <span className="subtle">· необязательно</span>
                  </label>
                  <input
                    id="offer-agreement-date"
                    disabled={occupied > 0}
                    type="date"
                    value={draft.agreementDate}
                    onChange={(e) => update({ agreementDate: e.target.value })}
                  />
                </div>
                <div className="form-field">
                  <label htmlFor="offer-places">Количество мест</label>
                  <input
                    id="offer-places"
                    type="number"
                    min={Math.max(1, occupied)}
                    max="1000"
                    value={draft.places}
                    onChange={(e) => update({ places: e.target.value })}
                  />
                </div>
              </div>
              <p className="field-hint">
                Время согласуете с посетителем отдельно. Пустая дата означает,
                что день тоже ещё не назначен.
              </p>
            </>
          ) : (
            <>
              <div className="booking-time-grid">
                <div className="form-field booking-time-date">
                  <label htmlFor="offer-date">Дата</label>
                  <input
                    id="offer-date"
                    disabled={occupied > 0}
                    type="date"
                    value={draft.date}
                    onChange={(e) =>
                      update({
                        date: e.target.value,
                        ...(draft.endDate === draft.date
                          ? { endDate: e.target.value }
                          : {}),
                      })
                    }
                  />
                </div>
                <div className="form-field">
                  <label htmlFor="offer-start">Начало</label>
                  <TimeInput
                    id="offer-start"
                    disabled={occupied > 0}
                    value={draft.start}
                    onChange={(start) => update({ start })}
                  />
                </div>
                <div className="form-field">
                  <label htmlFor="offer-end">Окончание</label>
                  <TimeInput
                    id="offer-end"
                    disabled={occupied > 0}
                    value={draft.end}
                    onChange={(end) => {
                      setAutoDuration(null);
                      update({ end });
                    }}
                  />
                </div>
                {draft.end &&
                  (draft.endDate !== draft.date ||
                    (Boolean(draft.start) && draft.end <= draft.start)) && (
                    <div className="form-field booking-time-date">
                      <label htmlFor="offer-end-date">Дата окончания</label>
                      <input
                        id="offer-end-date"
                        disabled={occupied > 0}
                        type="date"
                        value={draft.endDate}
                        onChange={(e) => {
                          setAutoDuration(null);
                          update({ endDate: e.target.value });
                        }}
                      />
                    </div>
                  )}
              </div>
              {!draft.end && (
                <p className="booking-limited-check">
                  <Icon name="clock" />
                  Без окончания проверим совпадения начала и известные
                  пересечения. Полную длительность проверить нельзя.
                </p>
              )}
            </>
          )}
          {calculationPending && !calculationError && (
            <p className="field-hint" role="status">
              Рассчитываем окончание по длительности заготовки…
            </p>
          )}
          {calculationError !== null && (
            <p className="field-error" role="alert">
              {bookingError(calculationError) ?? errorMessage(calculationError)}{' '}
              Можно выбрать ручное заполнение или изменить время окончания.
            </p>
          )}
          <div className="form-field">
            <label htmlFor="offer-state">Состояние талона</label>
            <select
              id="offer-state"
              value={draft.state}
              onChange={(e) => update({ state: e.target.value as OfferState })}
            >
              <option value="ready">В расписании</option>
              <option value="draft">Черновик</option>
              {initial && !copy && (
                <option value="withdrawn">Снят с расписания</option>
              )}
            </select>
          </div>
        </fieldset>
        {invalid && (
          <p className="field-error" role="alert">
            {invalid}
          </p>
        )}
        {conflict && (
          <div className="conflict-notice" role="alert">
            <p>Талон уже изменили. Ваш ввод сохранён в форме.</p>
            <button
              type="button"
              className="text-button"
              disabled={busy || Boolean(journal.pending)}
              onClick={() => {
                setDraft(offerDraft(current!, date));
                setVersion(current!.version);
                setNotice(null);
                setError(null);
                setAutoDuration(null);
              }}
            >
              Загрузить сохранённую версию
            </button>
          </div>
        )}
        {notice && (
          <div className="booking-overlap" role="alert">
            <strong>Есть пересечения: {notice.total}</strong>
            <p>
              Это могут быть альтернативные варианты времени. Проверьте список
              перед сохранением.
            </p>
            <ul>
              {notice.conflicts.map((c) => (
                <li key={c.id}>
                  {c.title || 'Приём'} · {timeLabel(c)} · {c.time_zone}
                </li>
              ))}
            </ul>
            {notice.total > notice.conflicts.length && (
              <p>Показаны первые {notice.conflicts.length} пересечений.</p>
            )}
            <p>
              Повторное сохранение подтвердит эти варианты. Если расписание
              изменится, потребуется новая проверка.
            </p>
          </div>
        )}
        {!notice && (
          <FormFailure
            error={error}
            pending={Boolean(journal.pending)}
            busy={busy}
            retry={() => {
              void save();
            }}
          />
        )}
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
            disabled={
              busy || Boolean(journal.pending) || conflict || calculationPending
            }
          >
            {busy
              ? 'Сохраняем…'
              : notice
                ? 'Сохранить как альтернативу'
                : 'Сохранить талон'}
          </button>
        </div>
      </form>
    </Modal>
  );
}
