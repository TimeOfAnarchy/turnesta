import { useEffect, useState } from 'react';
import type { FormEvent } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { ApiError, errorMessage, validateName } from './api';
import { Modal, Icon } from './components';
import { keys } from './queries';
import {
  bookingApi,
  BookingCommands,
  bookingError,
  conflictNotice,
  timeLabel,
} from './bookings';
import type {
  Specialist,
  Preset,
  Offer,
  OfferState,
  BookingCommand,
  BookingReceipt,
  ConflictNotice,
} from './bookings';

type SaveProps = {
  busy: boolean;
  journal: BookingCommands;
  run: (command?: BookingCommand) => Promise<BookingReceipt>;
  close: () => void;
};
function FormFailure({
  error,
  pending,
  retry,
  busy,
}: {
  error: unknown;
  pending: boolean;
  retry: () => void;
  busy: boolean;
}) {
  return (
    <>
      {error !== null && (
        <p className="field-error" role="alert">
          {bookingError(error) ?? errorMessage(error)}
        </p>
      )}
      {pending && !busy && (
        <div className="conflict-notice" role="alert">
          <p>
            Сохранение могло выполниться. Повторите прежний запрос, чтобы
            восстановить результат.
          </p>
          <button
            type="button"
            className="button secondary"
            disabled={busy}
            onClick={retry}
          >
            Повторить прежний запрос
          </button>
        </div>
      )}
    </>
  );
}
export function CatalogEditor({
  kind,
  initial,
  current,
  busy,
  journal,
  run,
  close,
}: SaveProps & {
  kind: 'specialist' | 'preset';
  initial: Specialist | Preset | null;
  current?: Specialist | Preset;
}) {
  const [id] = useState(() => initial?.id ?? crypto.randomUUID());
  const [name, setName] = useState(initial?.name ?? '');
  const [description, setDescription] = useState(
    initial && 'description' in initial ? initial.description : '',
  );
  const [minutes, setMinutes] = useState(
    initial &&
      'duration_minutes' in initial &&
      initial.duration_minutes !== null
      ? String(initial.duration_minutes)
      : '',
  );
  const [enabled, setEnabled] = useState(initial?.enabled ?? true);
  const [version, setVersion] = useState(initial?.version ?? 0);
  const [error, setError] = useState<unknown>(null);
  const [invalid, setInvalid] = useState<string>();
  const conflict = Boolean(current && current.version !== version);
  async function save(command?: BookingCommand) {
    try {
      await run(command);
      close();
    } catch (e) {
      setError(e);
    }
  }
  function submit(event: FormEvent) {
    event.preventDefault();
    if (busy || journal.pending || conflict) return;
    const invalid =
      validateName(name) ||
      (kind === 'specialist' &&
      ([...description.trim()].length > 500 ||
        /[\u0000-\u001f\u007f-\u009f]/u.test(description))
        ? 'Описание — до 500 символов, без переносов строк.'
        : undefined) ||
      (kind === 'preset' &&
      minutes !== '' &&
      (!/^\d+$/.test(minutes) || Number(minutes) < 1 || Number(minutes) > 1440)
        ? 'Длительность — от 1 до 1440 минут, либо оставьте поле пустым.'
        : undefined);
    setInvalid(invalid);
    if (invalid) return;
    void save(
      kind === 'specialist'
        ? {
            type: 'save_specialist',
            id,
            name,
            description,
            enabled,
            expected_version: version,
          }
        : {
            type: 'save_preset',
            id,
            name,
            duration_minutes: minutes === '' ? null : Number(minutes),
            enabled,
            expected_version: version,
          },
    );
  }
  return (
    <Modal
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
              />
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

type OfferDraft = {
  title: string;
  agreement: boolean;
  date: string;
  start: string;
  end: string;
  endDate: string;
  agreementDate: string;
  places: string;
  state: OfferState;
};
function offerDraft(
  item: Offer | null,
  date: string,
  copy = false,
): OfferDraft {
  return {
    title: item?.title ?? '',
    agreement: item?.mode === 'by_agreement',
    date: item?.date ?? date,
    start: item?.starts_local?.slice(11, 16) ?? '',
    end: item?.ends_local?.slice(11, 16) ?? '',
    endDate: item?.ends_local?.slice(0, 10) ?? item?.date ?? date,
    agreementDate: item?.mode === 'by_agreement' ? (item.date ?? '') : date,
    places: String(item?.places ?? 1),
    state:
      copy && item?.state === 'withdrawn' ? 'draft' : (item?.state ?? 'ready'),
  };
}
export function OfferEditor({
  tenant,
  location,
  initial,
  copy,
  current,
  specialist,
  date,
  zone,
  presets,
  morePresets,
  busy,
  journal,
  run,
  close,
}: SaveProps & {
  tenant: string;
  location: string;
  initial: Offer | null;
  copy?: boolean;
  current?: Offer;
  specialist: Specialist;
  date: string;
  zone: string;
  presets: Preset[];
  morePresets?: () => void;
}) {
  const [id] = useState(() =>
    initial && !copy ? initial.id : crypto.randomUUID(),
  );
  const [draft, setDraft] = useState(() => ({
    ...offerDraft(initial, date, copy),
    ...(!initial && !specialist.enabled ? { state: 'draft' as const } : {}),
  }));
  const [version, setVersion] = useState(copy ? 0 : (initial?.version ?? 0));
  const [presetId, setPresetId] = useState('');
  const [autoDuration, setAutoDuration] = useState<number | null>(null);
  const [notice, setNotice] = useState<ConflictNotice | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [invalid, setInvalid] = useState<string>();
  const conflict = Boolean(!copy && current && current.version !== version);
  const timezone = initial && !copy ? initial.time_zone : zone;
  const client = useQueryClient();
  const [calculatedFor, setCalculatedFor] = useState<string | null>(null);
  const [calculationError, setCalculationError] = useState<unknown>(null);
  const calculationKey =
    autoDuration !== null && !draft.agreement && draft.date && draft.start
      ? JSON.stringify([
          draft.date,
          draft.start,
          autoDuration,
          timezone,
          initial && !copy ? initial.id : null,
        ])
      : null;
  const calculationPending = Boolean(
    calculationKey && calculatedFor !== calculationKey,
  );
  useEffect(() => {
    setCalculationError(null);
    if (!calculationKey || autoDuration === null) return;
    const controller = new AbortController();
    let active = true;
    const timer = setTimeout(() => {
      void bookingApi
        .suggestTime(
          tenant,
          location,
          `${draft.date}T${draft.start}`,
          autoDuration,
          timezone,
          initial && !copy ? initial.id : null,
          controller.signal,
        )
        .then((result) => {
          if (!active) return;
          setDraft((old) => ({
            ...old,
            end: result.ends_local.slice(11, 16),
            endDate: result.ends_local.slice(0, 10),
          }));
          setNotice(null);
          setCalculatedFor(calculationKey);
        })
        .catch((error) => {
          if (!active) return;
          setCalculationError(error);
          if (
            error instanceof ApiError &&
            error.code === 'booking_time_zone_changed'
          )
            void client.invalidateQueries({ queryKey: keys.locations(tenant) });
        });
    }, 200);
    return () => {
      active = false;
      clearTimeout(timer);
      controller.abort();
    };
  }, [
    calculationKey,
    autoDuration,
    draft.date,
    draft.start,
    timezone,
    initial,
    copy,
    tenant,
    location,
    client,
  ]);
  function update(values: Partial<OfferDraft>) {
    setNotice(null);
    setError(null);
    setInvalid(undefined);
    setDraft((old) => ({ ...old, ...values }));
  }
  function choosePreset(value: string) {
    const selected = presets.find((p) => p.id === value);
    const previous = presets.find((p) => p.id === presetId);
    setPresetId(value);
    setAutoDuration(selected?.duration_minutes ?? null);
    if (selected)
      update({
        ...(draft.title === '' || draft.title === previous?.name
          ? { title: selected.name }
          : {}),
      });
  }
  async function save(command?: BookingCommand) {
    try {
      await run(command);
      close();
    } catch (e) {
      setNotice(conflictNotice(e));
      setError(e);
    }
  }
  function submit(event: FormEvent) {
    event.preventDefault();
    if (busy || journal.pending || conflict || calculationPending) return;
    const start = `${draft.date}T${draft.start}`,
      end = draft.end ? `${draft.endDate}T${draft.end}` : null;
    const invalid =
      [...draft.title.trim()].length > 120 ||
      /[\u0000-\u001f\u007f-\u009f]/u.test(draft.title)
        ? 'Название — до 120 символов, без управляющих знаков.'
        : draft.agreement
          ? !/^\d+$/.test(draft.places) ||
            Number(draft.places) < 1 ||
            Number(draft.places) > 1000
            ? 'Укажите от 1 до 1000 мест.'
            : undefined
          : !draft.date || !draft.start
            ? 'Укажите дату и время начала.'
            : end && (!draft.endDate || end <= start)
              ? 'Окончание должно быть позже начала. При необходимости измените дату окончания.'
              : undefined;
    setInvalid(invalid);
    if (invalid) return;
    void save({
      type: 'save_offer',
      id,
      specialist_id: initial && !copy ? initial.specialist_id : specialist.id,
      time_zone: timezone,
      title: draft.title,
      timing: draft.agreement
        ? {
            mode: 'by_agreement',
            date: draft.agreementDate || null,
            places: Number(draft.places),
          }
        : end
          ? { mode: 'interval', starts_local: start, ends_local: end }
          : { mode: 'start_only', starts_local: start },
      state: draft.state,
      expected_version: version,
      conflict_token: notice?.token ?? null,
    });
  }
  return (
    <Modal
      title={
        initial && !copy
          ? 'Изменить талон'
          : copy
            ? 'Похожий талон'
            : 'Новый талон'
      }
      description={`${specialist.name} · время ${timezone}. Заполняйте вручную; заготовка необязательна.`}
      busy={busy}
      close={close}
    >
      <form className="booking-form" onSubmit={submit} noValidate>
        <fieldset disabled={busy || Boolean(journal.pending)}>
          <div
            className="booking-mode"
            role="group"
            aria-label="Назначение времени"
          >
            <button
              type="button"
              aria-pressed={!draft.agreement}
              onClick={() => update({ agreement: false })}
            >
              <Icon name="clock" />
              Назначить время
            </button>
            <button
              type="button"
              aria-pressed={draft.agreement}
              onClick={() => update({ agreement: true })}
            >
              <Icon name="users" />
              По договорённости
            </button>
          </div>
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
          <details className="booking-preset-picker">
            <summary>
              Заполнить из заготовки{' '}
              <span className="subtle">· необязательно</span>
            </summary>
            <div className="form-field">
              <label htmlFor="offer-preset">Заготовка приёма</label>
              <select
                id="offer-preset"
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
          {draft.agreement ? (
            <>
              <div className="booking-form-grid">
                <div className="form-field">
                  <label htmlFor="offer-agreement-date">
                    Дата <span className="subtle">· необязательно</span>
                  </label>
                  <input
                    id="offer-agreement-date"
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
                    min="1"
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
              <div className="booking-form-grid">
                <div className="form-field">
                  <label htmlFor="offer-date">Дата начала</label>
                  <input
                    id="offer-date"
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
                  <input
                    id="offer-start"
                    type="time"
                    value={draft.start}
                    onChange={(e) => update({ start: e.target.value })}
                  />
                </div>
                <div className="form-field">
                  <label htmlFor="offer-end">
                    Окончание <span className="subtle">· необязательно</span>
                  </label>
                  <input
                    id="offer-end"
                    type="time"
                    value={draft.end}
                    onChange={(e) => {
                      setAutoDuration(null);
                      update({ end: e.target.value });
                    }}
                  />
                </div>
                {draft.end && (
                  <div className="form-field">
                    <label htmlFor="offer-end-date">Дата окончания</label>
                    <input
                      id="offer-end-date"
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
