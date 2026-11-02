import { useEffect, useRef, useState } from 'react';
import { Modal } from './components';
import { ApiError, errorMessage } from './api';
import { batchPreview } from './bookingBatch';
import type { BatchRequest, BatchPreview, BatchSource } from './bookingBatch';
import {
  bookingError,
  shiftDate,
  timeLabel,
  offerStateLabels,
} from './bookings';
import type {
  BookingCommand,
  BookingReceipt,
  BookingCommands,
  Specialist,
  Preset,
} from './bookings';

export function BookingBatchEditor({
  tenant,
  location,
  specialist,
  zone,
  date,
  presets,
  morePresets,
  busy,
  journal,
  run,
  close,
}: {
  tenant: string;
  location: string;
  specialist: Specialist;
  zone: string;
  date: string;
  presets: Preset[];
  morePresets?: () => void;
  busy: boolean;
  journal: BookingCommands;
  run: (command?: BookingCommand) => Promise<BookingReceipt>;
  close: () => void;
}) {
  const [id] = useState(() => crypto.randomUUID());
  const [mode, setMode] = useState<'copy_day' | 'preset_series'>('copy_day');
  const [sourceDate, setSourceDate] = useState(date);
  const [dates, setDates] = useState([shiftDate(date, 1)]),
    [pick, setPick] = useState(shiftDate(date, 2));
  const [presetId, setPresetId] = useState(
    presets.find((p) => p.enabled && p.duration_minutes)?.id ?? '',
  );
  const [start, setStart] = useState(''),
    [end, setEnd] = useState(''),
    [gap, setGap] = useState('0');
  const [state, setState] = useState<'ready' | 'draft'>('ready');
  const [preview, setPreview] = useState<BatchPreview | null>(null),
    [request, setRequest] = useState<BatchRequest | null>(null);
  const [checking, setChecking] = useState(false),
    [allow, setAllow] = useState(false),
    [error, setError] = useState<unknown>(null);
  const controller = useRef<AbortController | null>(null),
    generation = useRef(0);
  const previewHeading = useRef<HTMLHeadingElement | null>(null);
  useEffect(() => {
    if (preview) previewHeading.current?.focus();
  }, [preview]);
  const locked = busy || checking || Boolean(journal.pending);
  useEffect(
    () => () => {
      generation.current++;
      controller.current?.abort();
    },
    [],
  );
  useEffect(() => {
    generation.current++;
    controller.current?.abort();
    setChecking(false);
    setPreview(null);
    setRequest(null);
    setAllow(false);
  }, [zone, specialist.id, specialist.version]);
  function change(action: () => void) {
    if (locked) return;
    action();
    setPreview(null);
    setRequest(null);
    setAllow(false);
    setError(null);
  }
  function addDate() {
    if (!pick || dates.includes(pick) || dates.length >= 31) return;
    change(() => {
      setDates([...dates, pick].sort());
      setPick(shiftDate(pick, 1));
    });
  }
  async function check() {
    if (locked || !dates.length) return;
    setError(null);
    setPreview(null);
    setAllow(false);
    setChecking(true);
    const mine = ++generation.current;
    controller.current?.abort();
    controller.current = new AbortController();
    const source: BatchSource =
      mode === 'copy_day'
        ? { kind: mode, source_date: sourceDate, target_dates: dates }
        : {
            kind: mode,
            preset_id: presetId,
            dates,
            start_time: start,
            end_time: end,
            gap_minutes: Number(gap),
          };
    const body: BatchRequest = {
      id,
      specialist_id: specialist.id,
      time_zone: zone,
      state,
      source,
    };
    try {
      const result = await batchPreview(
        tenant,
        location,
        body,
        controller.current.signal,
      );
      if (mine === generation.current) {
        setPreview(result);
        setRequest(body);
      }
    } catch (e) {
      if (mine === generation.current) setError(e);
    } finally {
      if (mine === generation.current) setChecking(false);
    }
  }
  async function save(recover = false) {
    try {
      if (recover) await run();
      else if (preview && request)
        await run({
          type: 'save_batch',
          request,
          preview_token: preview.token,
          allow_alternatives: allow,
        });
      else return;
      close();
    } catch (e) {
      setError(e);
      if (
        e instanceof ApiError &&
        [
          'batch_preview_changed',
          'batch_duplicate',
          'batch_time',
          'booking_time_zone_changed',
          'specialist_disabled',
        ].includes(e.code)
      ) {
        setPreview(null);
        setRequest(null);
        setAllow(false);
      }
    }
  }
  const preset = presets.find((p) => p.id === presetId),
    canCheck =
      mode === 'copy_day'
        ? Boolean(sourceDate)
        : Boolean(
            preset?.enabled &&
              preset.duration_minutes &&
              start &&
              end &&
              Number.isInteger(Number(gap)) &&
              Number(gap) >= 0,
          );
  return (
    <Modal
      icon="calendar"
      title="Заполнить на несколько дней"
      description={`${specialist.name} · ${zone}. Сначала проверьте будущие талоны; клиенты не копируются.`}
      close={close}
      busy={busy || checking}
    >
      <form
        className="booking-form booking-batch"
        onSubmit={(e) => {
          e.preventDefault();
          void check();
        }}
      >
        <fieldset disabled={locked}>
          <div
            className="segmented"
            role="group"
            aria-label="Способ заполнения"
          >
            <button
              type="button"
              aria-pressed={mode === 'copy_day'}
              onClick={() => change(() => setMode('copy_day'))}
            >
              Копировать день
            </button>
            <button
              type="button"
              aria-pressed={mode === 'preset_series'}
              onClick={() => change(() => setMode('preset_series'))}
            >
              Серия из заготовки
            </button>
          </div>
          {mode === 'copy_day' ? (
            <>
              <label htmlFor="batch-source-date">День-образец</label>
              <input
                id="batch-source-date"
                type="date"
                required
                value={sourceDate}
                onChange={(e) => change(() => setSourceDate(e.target.value))}
              />
              <p className="field-hint">
                Все талоны этого дня, включая черновики, но без снятых и без
                даты. Названия, местное время и лимиты сохраняются. Состояние
                новых талонов выбирается ниже.
              </p>
            </>
          ) : (
            <>
              <label htmlFor="batch-preset">Заготовка приёма</label>
              <select
                id="batch-preset"
                required
                value={presetId}
                onChange={(e) => change(() => setPresetId(e.target.value))}
              >
                <option value="">Выберите заготовку</option>
                {presets.map((p) => (
                  <option
                    key={p.id}
                    value={p.id}
                    disabled={!p.enabled || !p.duration_minutes}
                  >
                    {p.name}
                    {p.duration_minutes
                      ? ` · ${p.duration_minutes} мин`
                      : ' · без длительности'}
                  </option>
                ))}
              </select>
              {morePresets && (
                <button
                  className="text-button"
                  type="button"
                  onClick={morePresets}
                >
                  Ещё заготовки
                </button>
              )}
              <div className="booking-batch-window">
                <div>
                  <label htmlFor="batch-start">Начало периода</label>
                  <input
                    id="batch-start"
                    type="time"
                    required
                    value={start}
                    onChange={(e) => change(() => setStart(e.target.value))}
                  />
                </div>
                <div>
                  <label htmlFor="batch-end">Конец периода</label>
                  <input
                    id="batch-end"
                    type="time"
                    required
                    value={end}
                    onChange={(e) => change(() => setEnd(e.target.value))}
                  />
                </div>
                <div>
                  <label htmlFor="batch-gap">Перерыв, мин</label>
                  <input
                    id="batch-gap"
                    type="number"
                    required
                    min="0"
                    max="1440"
                    value={gap}
                    onChange={(e) => change(() => setGap(e.target.value))}
                  />
                </div>
              </div>
              <p className="field-hint">
                Период в пределах одного дня. Длительность и перерыв считаются в
                реально прошедших минутах. Неполный приём в конце периода не
                создаётся. Время можно задать вручную в обычном редакторе.
              </p>
            </>
          )}
          <div className="booking-batch-dates">
            <label htmlFor="batch-target-date">Даты для новых талонов</label>
            <div className="booking-batch-date-add">
              <input
                id="batch-target-date"
                type="date"
                value={pick}
                onChange={(e) => setPick(e.target.value)}
              />
              <button
                type="button"
                className="button secondary"
                disabled={!pick || dates.includes(pick) || dates.length >= 31}
                onClick={addDate}
              >
                Добавить дату
              </button>
            </div>
            <ul aria-label="Выбранные даты">
              {dates.map((d) => (
                <li key={d}>
                  <span>
                    {new Date(`${d}T12:00:00Z`).toLocaleDateString('ru-RU', {
                      timeZone: 'UTC',
                      day: 'numeric',
                      month: 'short',
                      weekday: 'short',
                    })}
                  </span>
                  <button
                    type="button"
                    aria-label={`Убрать дату ${d}`}
                    onClick={() =>
                      change(() => setDates(dates.filter((v) => v !== d)))
                    }
                  >
                    ×
                  </button>
                </li>
              ))}
            </ul>
            <p className="field-hint">
              Выбрано дат: {dates.length} из 31. До 200 новых талонов за одно
              сохранение.
            </p>
          </div>
          <label htmlFor="batch-state">Состояние новых талонов</label>
          <select
            id="batch-state"
            value={state}
            onChange={(e) =>
              change(() => setState(e.target.value as 'ready' | 'draft'))
            }
          >
            <option value="ready">В расписании</option>
            <option value="draft">Черновик</option>
          </select>
        </fieldset>
        {error !== null && (
          <p className="field-error" role="alert">
            {bookingError(error) ?? errorMessage(error)}
          </p>
        )}
        {journal.pending && (
          <div className="conflict-notice" role="alert">
            <p>
              Результат сохранения неизвестен. Восстановите исходный пакет —
              повтор не добавит талоны второй раз.
            </p>
            <button
              type="button"
              className="button secondary"
              disabled={busy}
              onClick={() => void save(true)}
            >
              Восстановить пакет
            </button>
          </div>
        )}
        <div className="form-actions">
          <button
            className="button secondary"
            type="submit"
            disabled={locked || !canCheck || dates.length === 0}
          >
            {checking ? 'Проверяем…' : 'Предпросмотр'}
          </button>
        </div>
        {preview && (
          <section
            className="booking-batch-preview"
            aria-label="Будущие талоны"
          >
            <h3 ref={previewHeading} tabIndex={-1}>
              Предпросмотр расписания
            </h3>
            <div className="booking-batch-summary">
              <strong>Будет добавлено: {preview.rows.length}</strong>
              <span>{offerStateLabels[state]}</span>
            </div>
            <p className="field-hint">
              Сохранение целиком, без замены существующих талонов. Если
              расписание или записи изменятся, потребуется новый предпросмотр.
            </p>
            {preview.has_duplicates && (
              <p className="field-error" role="alert">
                Найдены точные дубликаты. Измените выбранные даты или источник;
                такой пакет сохранить нельзя.
              </p>
            )}
            <ol
              className="booking-batch-rows"
              tabIndex={0}
              aria-label="Список будущих талонов"
            >
              {preview.rows.map((r) => {
                const issues = preview.conflicts.filter(
                  (c) => c.row === r.index || c.other_row === r.index,
                );
                return (
                  <li
                    key={r.index}
                    className={issues.length ? 'batch-row-warning' : ''}
                  >
                    <div>
                      <span className="batch-row-index">№{r.index}</span>
                      <strong>{r.date}</strong>
                      <span>{timeLabel(r)}</span>
                      <span>{r.title || 'Приём'}</span>
                      {r.mode === 'by_agreement' && (
                        <small>Лимит мест: {r.places}</small>
                      )}
                    </div>
                    {issues.map((c, i) => (
                      <small key={i} className="batch-conflict-label">
                        {c.kind === 'duplicate'
                          ? 'Такой талон уже есть'
                          : c.kind === 'internal'
                            ? `Пересечение с талоном №${c.row === r.index ? c.other_row : c.row}`
                            : c.kind === 'booked'
                              ? 'Специалист занят клиентом'
                              : `Пересекается: ${c.title || 'Приём'}`}
                        {c.kind !== 'internal' && c.starts_local
                          ? ` · ${c.starts_local.slice(11)}`
                          : ''}
                      </small>
                    ))}
                  </li>
                );
              })}
            </ol>
            {preview.conflicts_truncated && (
              <p className="field-hint">
                Показаны первые 100 пересечений. Уменьшите пакет, чтобы
                проверить детали остальных.
              </p>
            )}
            {preview.conflicts.length > 0 && !preview.has_duplicates && (
              <label className="checkbox-field">
                <input
                  type="checkbox"
                  checked={allow}
                  disabled={locked}
                  onChange={(e) => setAllow(e.target.checked)}
                />
                <span>
                  Добавить пересекающиеся предложения как альтернативы.
                  Существующие записи сохранятся; двойная запись клиентов не
                  разрешается.
                </span>
              </label>
            )}
            <div className="form-actions">
              <button
                type="button"
                className="button primary"
                disabled={
                  locked ||
                  preview.has_duplicates ||
                  preview.conflicts_truncated ||
                  (preview.conflicts.length > 0 && !allow)
                }
                onClick={() => void save()}
              >
                Добавить талоны · {preview.rows.length}
              </button>
            </div>
          </section>
        )}
      </form>
    </Modal>
  );
}
