import { useState } from 'react';
import type { FormEvent } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ApiError } from './api';
import { ErrorNotice, Icon, Loading, SuccessNotice } from './components';
import { bookingWidgetApi, widgetSnippet } from './bookingWidget';
import type { WidgetSettings } from './bookingWidget';

export function BookingWidgetSettings({
  tenant,
  location,
  canManage,
}: {
  tenant: string;
  location: string;
  canManage: boolean;
}) {
  const query = useQuery({
    queryKey: ['organization', tenant, 'bookings', location, 'widget'],
    queryFn: ({ signal }) =>
      bookingWidgetApi.settings(tenant, location, signal),
    retry: false,
    refetchInterval: 10_000,
  });
  return (
    <section className="booking-widget-settings" aria-label="Виджет на сайте">
      <div className="booking-panel-head">
        <span className="square-icon" aria-hidden="true">
          <Icon name="external" />
        </span>
        <div>
          <h2>Виджет на вашем сайте</h2>
          <p>
            Календарь записи прямо на странице сайта. Укажите разрешённые
            сайты и скопируйте код в HTML.
          </p>
        </div>
      </div>
      {query.isPending ? (
        <Loading label="Загружаем настройки виджета…" />
      ) : query.data ? (
        <WidgetEditor
          key={`${tenant}:${location}:${canManage}`}
          tenant={tenant}
          location={location}
          current={query.data}
          canManage={canManage}
        />
      ) : null}
      {query.isError && (
        <ErrorNotice error={query.error} retry={() => void query.refetch()} />
      )}
    </section>
  );
}
function WidgetEditor({
  tenant,
  location,
  current,
  canManage,
}: {
  tenant: string;
  location: string;
  current: WidgetSettings;
  canManage: boolean;
}) {
  const queryClient = useQueryClient();
  const [baseline, setBaseline] = useState(current),
    [origins, setOrigins] = useState(current.allowed_origins.join('\n'));
  const [busy, setBusy] = useState(false),
    [uncertain, setUncertain] = useState(false),
    [saved, setSaved] = useState(false),
    [copied, setCopied] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const changed = current.version !== baseline.version;
  const key = ['organization', tenant, 'bookings', location, 'widget'];
  const snippet = current.public_url
    ? widgetSnippet(current.public_url, current.script_url)
    : null;
  function accept(value: WidgetSettings) {
    setBaseline(value);
    setOrigins(value.allowed_origins.join('\n'));
    setUncertain(false);
    setError(null);
  }
  async function reload() {
    setBusy(true);
    try {
      const v = await bookingWidgetApi.settings(
        tenant,
        location,
        AbortSignal.timeout(10_000),
      );
      accept(v);
      queryClient.setQueryData(key, v);
    } catch (e) {
      setError(e);
    } finally {
      setBusy(false);
    }
  }
  async function save(event: FormEvent) {
    event.preventDefault();
    if (busy || changed || uncertain || !canManage) return;
    setBusy(true);
    setError(null);
    setSaved(false);
    try {
      const v = await bookingWidgetApi.configure(
        tenant,
        location,
        origins
          .split(/\r?\n/)
          .map((s) => s.trim())
          .filter(Boolean),
        baseline.version,
      );
      accept(v);
      queryClient.setQueryData(key, v);
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
      await navigator.clipboard.writeText(snippet!);
      setCopied(true);
    } catch (e) {
      setError(e);
    }
  }
  return (
    <>
      {canManage ? (
        <form onSubmit={(event) => void save(event)} className="booking-form">
          <fieldset disabled={busy || uncertain} className="form-field">
            <label htmlFor={`widget-origins-${location}`}>
              Разрешённые сайты, каждый с новой строки
            </label>
            <textarea
              id={`widget-origins-${location}`}
              rows={3}
              value={origins}
              maxLength={10_240}
              placeholder={'https://clinic.ru\nhttps://www.clinic.ru'}
              onChange={(e) => {
                setOrigins(e.target.value);
                setSaved(false);
              }}
              aria-describedby={`widget-origins-hint-${location}`}
            />
            <p id={`widget-origins-hint-${location}`} className="field-hint">
              До 10 адресов с https://, без пути и звёздочек. clinic.ru и
              www.clinic.ru — разные сайты. Пустой список запрещает новое
              встраивание. Для локальной разработки разрешён http://localhost с
              портом. Сайт должен отличаться от адреса Turnesta.
            </p>
          </fieldset>
          {changed && (
            <p className="notice">
              Настройки изменили в другом окне. Загрузите сохранённую версию.
            </p>
          )}
          {uncertain && (
            <p className="notice">
              Результат сохранения неизвестен. Перечитайте настройки перед новой
              правкой.
            </p>
          )}
          <div className="form-actions">
            {(changed || uncertain) && (
              <button
                type="button"
                className="button secondary"
                disabled={busy}
                onClick={() => void reload()}
              >
                Загрузить настройки виджета
              </button>
            )}
            <button
              type="submit"
              className="button secondary"
              disabled={busy || changed || uncertain}
            >
              {busy ? 'Сохраняем…' : 'Сохранить сайты'}
            </button>
          </div>
        </form>
      ) : (
        <p className="field-hint">
          Разрешённые сайты:{' '}
          {current.allowed_origins.join(', ') || 'пока не настроены'}. Изменения
          доступны владельцу и управляющему.
        </p>
      )}
      {error instanceof ApiError && error.code === 'invalid_widget_origins' ? (
        <p className="notice" role="alert">
          Проверьте адреса: нужен точный HTTPS-адрес сайта без пути, параметров
          и звёздочек. Адрес самого Turnesta не подходит.
        </p>
      ) : error ? (
        <ErrorNotice error={error} />
      ) : null}
      {saved && <SuccessNotice>Сайты для встраивания сохранены.</SuccessNotice>}
      {snippet && current.allowed_origins.length > 0 ? (
        <div className="booking-widget-code form-field">
          <label htmlFor={`widget-code-${location}`}>Код для сайта</label>
          <textarea
            id={`widget-code-${location}`}
            rows={6}
            readOnly
            value={snippet}
            spellCheck={false}
          />
          <button
            type="button"
            className="button secondary"
            onClick={() => void copy()}
          >
            {copied ? 'Код скопирован' : 'Копировать код'}
          </button>
          <p className="field-hint">
            Вставьте блок в нужное место страницы. Скрипт достаточно подключить
            один раз. Если у сайта есть Content Security Policy, разрешите адрес
            Turnesta в script-src и frame-src. Просмотр в редакторе сайта может
            отличаться от опубликованной страницы.
          </p>
        </div>
      ) : (
        <p className="field-hint">
          Сначала сохраните публичную ссылку в настройках онлайн-записи и
          добавьте разрешённый сайт. После этого здесь появится код.
        </p>
      )}
      <p className="field-hint">
        При недоступности виджета остаётся ссылка на отдельную страницу. История
        записей внутри сайта может отличаться от отдельной страницы из-за
        настроек браузера.
      </p>
    </>
  );
}
