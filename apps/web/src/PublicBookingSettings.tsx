import { useState } from 'react';
import type { FormEvent } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ApiError, errorMessage } from './api';
import { ErrorNotice, Icon, Loading, SuccessNotice } from './components';
import { BookingWidgetSettings } from './BookingWidgetSettings';
import {
  publicBookingApi,
  fieldKindLabels,
  newFormField,
} from './publicBookings';
import type {
  BookingPublicSettings,
  BookingForm,
  BookingFieldKind,
} from './publicBookings';

export function PublicBookingSettings({
  tenant,
  location,
  canManage,
}: {
  tenant: string;
  location: string;
  canManage: boolean;
}) {
  const key = ['organization', tenant, 'bookings', location, 'public-access'];
  const query = useQuery({
    queryKey: key,
    queryFn: ({ signal }) =>
      publicBookingApi.settings(tenant, location, signal),
    retry: false,
    refetchInterval: 10_000,
  });
  return (
    <details className="panel booking-public-settings">
      <summary>
        <Icon name="globe" />
        <span>
          <strong>Онлайн-запись</strong>
          <span>Публичная ссылка и анкета посетителя</span>
        </span>
        <span className="booking-state">
          {query.data?.enabled ? 'Включена' : 'Выключена'}
        </span>
        <Icon name="chevron" />
      </summary>
      <div className="booking-public-body">
        {query.isPending ? (
          <Loading label="Загружаем настройки…" />
        ) : query.data ? (
          <>
            <PublicSettingsEditor
              key={`${tenant}:${location}:${canManage}`}
              tenant={tenant}
              location={location}
              initial={query.data}
              current={query.data}
              canManage={canManage}
            />
            {query.isError && (
              <ErrorNotice
                error={query.error}
                retry={() => void query.refetch()}
              />
            )}
          </>
        ) : (
          <ErrorNotice error={query.error} retry={() => void query.refetch()} />
        )}
      </div>
      <div className="booking-public-body">
        <BookingWidgetSettings
          tenant={tenant}
          location={location}
          canManage={canManage}
        />
      </div>
    </details>
  );
}
function PublicSettingsEditor({
  tenant,
  location,
  initial,
  current,
  canManage,
}: {
  tenant: string;
  location: string;
  initial: BookingPublicSettings;
  current: BookingPublicSettings;
  canManage: boolean;
}) {
  const queryClient = useQueryClient();
  const [baseline, setBaseline] = useState(initial);
  const [form, setForm] = useState<BookingForm>(() =>
    structuredClone(initial.form),
  );
  const [enabled, setEnabled] = useState(initial.enabled);
  const [busy, setBusy] = useState(false),
    [uncertain, setUncertain] = useState(false);
  const [error, setError] = useState<unknown>(null),
    [saved, setSaved] = useState(false),
    [copied, setCopied] = useState(false);
  const changed = current.version !== baseline.version;
  function update(
    index: number,
    value: Partial<BookingForm['fields'][number]>,
  ) {
    setSaved(false);
    setForm((old) => ({
      fields: old.fields.map((f, i) => (i === index ? { ...f, ...value } : f)),
    }));
  }
  async function reload() {
    setBusy(true);
    try {
      const value = await publicBookingApi.settings(
        tenant,
        location,
        AbortSignal.timeout(10_000),
      );
      setBaseline(value);
      setForm(structuredClone(value.form));
      setEnabled(value.enabled);
      setUncertain(false);
      setError(null);
      await queryClient.invalidateQueries({
        queryKey: [
          'organization',
          tenant,
          'bookings',
          location,
          'public-access',
        ],
      });
    } catch (e) {
      setError(e);
    } finally {
      setBusy(false);
    }
  }
  async function submit(event: FormEvent) {
    event.preventDefault();
    if (busy || changed || uncertain || !canManage) return;
    setBusy(true);
    setError(null);
    setSaved(false);
    try {
      const result = await publicBookingApi.configure(
        tenant,
        location,
        enabled,
        {
          fields: form.fields.map((field) => ({
            ...field,
            options: field.options.map((value) => value.trim()).filter(Boolean),
          })),
        },
        baseline.version,
      );
      setBaseline(result);
      setForm(structuredClone(result.form));
      setEnabled(result.enabled);
      await queryClient.invalidateQueries({
        queryKey: [
          'organization',
          tenant,
          'bookings',
          location,
          'public-access',
        ],
      });
      setSaved(true);
    } catch (e) {
      setError(e);
      if (!(e instanceof ApiError) || e.status >= 500 || e.status === 0)
        setUncertain(true);
    } finally {
      setBusy(false);
    }
  }
  async function copy() {
    try {
      await navigator.clipboard.writeText(current.public_url!);
      setCopied(true);
    } catch (e) {
      setError(e);
    }
  }
  return (
    <>
      {current.public_url && (
        <div className="booking-public-link">
          <div>
            <strong>Ссылка для посетителей</strong>
            <a href={current.public_url} target="_blank" rel="noreferrer">
              {current.public_url}
            </a>
          </div>
          <button
            className="button secondary"
            disabled={busy}
            type="button"
            onClick={() => void copy()}
          >
            {copied ? 'Скопировано' : 'Копировать'}
          </button>
        </div>
      )}
      {!canManage ? (
        <p className="field-hint">
          Владелец или управляющий настраивает публикацию и поля анкеты.
        </p>
      ) : (
        <form onSubmit={(event) => void submit(event)} className="booking-form">
          <fieldset disabled={busy || uncertain}>
            <label className="checkbox-field">
              <input
                type="checkbox"
                checked={enabled}
                onChange={(event) => {
                  setEnabled(event.target.checked);
                  setSaved(false);
                }}
              />
              <span>Принимать записи через публичную страницу</span>
            </label>
            <p className="field-hint">
              Включение публикует названия и описания включённых специалистов
              этой точки и свободные талоны «В расписании». Контакты клиентов не
              показываются. Выключение остановит новые онлайн-записи и сохранит
              уже созданные.
            </p>
            <h3 className="booking-form-settings-title">Анкета посетителя</h3>
            <p className="field-hint">
              Имя всегда обязательно. Остальные поля, подписи и обязательность
              настраиваются. Изменение анкеты применяется к новым записям.
            </p>
            <div className="booking-form-fields">
              {form.fields.map((field, index) => (
                <section
                  key={field.key}
                  className="booking-form-field-config"
                  aria-label={`Поле ${field.label || fieldKindLabels[field.kind]}`}
                >
                  <div className="booking-config-field-head">
                    <span className="booking-state">
                      {fieldKindLabels[field.kind]}
                    </span>
                    {field.key !== 'client_name' && (
                      <button
                        className="text-button"
                        type="button"
                        aria-label={`Убрать поле ${field.label || fieldKindLabels[field.kind]}`}
                        onClick={() => {
                          setForm((old) => ({
                            fields: old.fields.filter((_, i) => i !== index),
                          }));
                          setSaved(false);
                        }}
                      >
                        Убрать
                      </button>
                    )}
                  </div>
                  <div className="form-field">
                    <label htmlFor={`public-field-label-${field.key}`}>
                      Подпись поля
                    </label>
                    <input
                      id={`public-field-label-${field.key}`}
                      value={field.label}
                      maxLength={120}
                      required
                      onChange={(event) =>
                        update(index, { label: event.target.value })
                      }
                    />
                  </div>
                  <label className="checkbox-field">
                    <input
                      type="checkbox"
                      checked={field.required}
                      disabled={field.key === 'client_name'}
                      onChange={(event) =>
                        update(index, { required: event.target.checked })
                      }
                    />
                    Обязательное поле
                  </label>
                  {field.kind === 'select' && (
                    <div className="form-field">
                      <label htmlFor={`public-options-${field.key}`}>
                        Варианты ответа, каждый с новой строки
                      </label>
                      <textarea
                        id={`public-options-${field.key}`}
                        rows={3}
                        value={field.options.join('\n')}
                        onChange={(event) =>
                          update(index, {
                            options: event.target.value.split('\n'),
                          })
                        }
                      />
                      <span className="field-hint">
                        До 10 разных вариантов, до 120 символов каждый.
                      </span>
                    </div>
                  )}
                </section>
              ))}
            </div>
            <div className="form-field booking-add-field">
              <label htmlFor={`public-add-field-${location}`}>
                Добавить поле
              </label>
              <select
                id={`public-add-field-${location}`}
                value=""
                disabled={form.fields.length >= 14}
                onChange={(event) => {
                  if (event.target.value) {
                    setForm((old) => ({
                      fields: [
                        ...old.fields,
                        newFormField(event.target.value as BookingFieldKind),
                      ],
                    }));
                    setSaved(false);
                  }
                }}
              >
                <option value="">Выберите тип поля</option>
                {(['phone', 'email', 'note', 'text', 'select'] as const)
                  .filter(
                    (kind) =>
                      ['text', 'select'].includes(kind) ||
                      !form.fields.some((f) => f.key === kind),
                  )
                  .map((kind) => (
                    <option key={kind} value={kind}>
                      {fieldKindLabels[kind]}
                    </option>
                  ))}
              </select>
              <span className="field-hint">Всего до 14 полей.</span>
            </div>
          </fieldset>
          {changed && (
            <p className="conflict-notice" role="alert">
              Настройки уже изменили. Ваш черновик сохранён в форме; загрузите
              актуальную версию перед повтором.
            </p>
          )}
          {uncertain && (
            <p className="conflict-notice" role="alert">
              Сохранение могло выполниться. Загрузите сохранённую версию перед
              новым изменением.
            </p>
          )}
          {error !== null && (
            <p className="error-notice" role="alert">
              {error instanceof ApiError &&
              error.code === 'invalid_booking_form'
                ? 'Проверьте подписи, обязательное имя и варианты ответа.'
                : errorMessage(error)}
            </p>
          )}
          {(changed ||
            uncertain ||
            (error instanceof ApiError &&
              error.code === 'version_conflict')) && (
            <button
              className="text-button"
              type="button"
              disabled={busy}
              onClick={() => void reload()}
            >
              Загрузить сохранённую версию настроек
            </button>
          )}
          {saved && (
            <SuccessNotice>Настройки онлайн-записи сохранены.</SuccessNotice>
          )}
          <div className="dialog-actions">
            <button
              className="button primary"
              disabled={busy || changed || uncertain}
            >
              {busy ? 'Сохраняем…' : 'Сохранить настройки записи'}
            </button>
          </div>
        </form>
      )}
    </>
  );
}
