import {
  PublicBookingClient,
  bookingRequest,
  guestError,
  shiftMonth,
  monthGrid,
  dateLabel,
  timeLabel,
  offsetLabel,
} from './bookings-client.js';
import { connectBookingEmbed } from './bookings-embed.js';
import { probeBookingStorage } from './booking-widget-protocol.js';

const app = document.querySelector('#booking-app'),
  root = document.querySelector('#booking-root');
if (app && root && (await connectBookingEmbed(app)))
  start(app.dataset.slug, root);
function node(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const child of children.flat(Infinity)) {
    if (child !== null && child !== undefined && child !== false)
      el.append(
        child instanceof Node ? child : document.createTextNode(String(child)),
      );
  }
  if (tag === 'button') el.type = attrs.type ?? 'button';
  for (const [key, value] of Object.entries(attrs)) {
    if (key.startsWith('on'))
      el.addEventListener(key.slice(2).toLowerCase(), value);
    else if (
      ['disabled', 'required', 'checked', 'hidden', 'open'].includes(key)
    )
      el[key] = Boolean(value);
    else if (key === 'class') el.className = value;
    else if (key === 'value') el.value = value ?? '';
    else if (value !== undefined && value !== null)
      el.setAttribute(key, String(value));
  }
  if (tag === 'button' || tag === 'summary')
    el.dataset.focusKey ??= attrs['aria-label'] ?? el.textContent;
  return el;
}
async function start(slug, root) {
  const s = {
    profile: null,
    month: '',
    date: '',
    specialist: '',
    view: 'calendar',
    days: [],
    items: [],
    cursor: null,
    version: null,
    selected: null,
    step: 'calendar',
    answers: {},
    error: null,
    notice: '',
    loading: true,
    busy: false,
    mustReview: false,
    receipt: null,
    receiptToken: null,
    savedOpen: false,
    snapshot: { pending: null, history: [] },
    storageError: null,
  };
  let client;
  let generation = 0;
  let controller;
  try {
    await probeBookingStorage(localStorage, navigator.locks);
    client = new PublicBookingClient(slug, localStorage, navigator.locks);
    s.snapshot = client.read();
  } catch (e) {
    s.storageError = e;
  }
  const base = `/api/public/bookings/${encodeURIComponent(slug)}`;
  const locked = () => s.busy || Boolean(s.snapshot.pending);
  function refreshSnapshot() {
    try {
      if (client) s.snapshot = client.read();
    } catch (e) {
      s.storageError = e;
    }
  }
  function focusTitle() {
    requestAnimationFrame(() => root.querySelector('h2')?.focus());
  }
  function summary(item) {
    return node(
      'div',
      { class: 'summary' },
      node('strong', {}, `${dateLabel(item.date)} · ${timeLabel(item)}`),
      node('p', {}, `${item.specialist_name} · ${item.title || 'Приём'}`),
      node(
        'p',
        {},
        `${item.time_zone}${item.starts_at ? ` · ${offsetLabel(item.starts_at, item.time_zone)}` : ''}`,
      ),
    );
  }
  function errorBox(error) {
    return node('div', { class: 'error', role: 'alert' }, guestError(error));
  }
  async function load() {
    s.loading = true;
    s.error = null;
    render();
    try {
      const profile = await bookingRequest(base);
      s.profile = profile;
      document.title = `Запись · ${profile.location_name} · Turnesta`;
      if (!s.month) {
        s.month = profile.today.slice(0, 7);
        s.date = profile.today;
        if (profile.specialists.length === 1)
          s.specialist = profile.specialists[0].id;
      }
      if (profile.enabled) await refresh();
    } catch (e) {
      s.error = e;
    } finally {
      s.loading = false;
      render();
    }
  }
  async function refresh(more = false, quiet = false) {
    const mine = ++generation;
    controller?.abort();
    controller = new AbortController();
    let repaint = !quiet;
    if (!quiet) {
      s.loading = true;
      s.error = null;
      render();
    }
    const query = new URLSearchParams({
      month: s.month,
      date: s.date,
      limit: '50',
      ...(s.specialist ? { specialist_id: s.specialist } : {}),
      ...(more && s.cursor ? { after: s.cursor, version: s.version } : {}),
    });
    try {
      const value = await bookingRequest(`${base}/availability?${query}`, {
        signal: controller.signal,
      });
      if (mine !== generation) return;
      if (quiet) {
        if (s.step !== 'calendar' || s.selected || locked()) return;
        if (value.version === s.version && !s.error) return;
      }
      repaint = true;
      s.error = null;
      s.profile = value.profile;
      s.days = value.days;
      s.items = more
        ? [
            ...new Map(
              [...s.items, ...value.items].map((i) => [i.id, i]),
            ).values(),
          ]
        : value.items;
      s.cursor = value.next_cursor;
      s.version = value.version;
      if (
        s.selected &&
        !s.items.some(
          (i) => i.id === s.selected.id && i.version === s.selected.version,
        )
      ) {
        s.selected = null;
        s.notice = 'Выбранное время изменилось. Выберите талон ещё раз.';
      }
    } catch (e) {
      if (mine !== generation) return;
      if (quiet && (s.step !== 'calendar' || s.selected || locked())) return;
      repaint = true;
      if (more && e.code === 'booking_changed') {
        s.notice = 'Расписание обновилось. Показываем актуальные талоны.';
        s.selected = null;
        s.cursor = null;
        await refresh();
        return;
      }
      s.items = [];
      s.cursor = null;
      s.error = e;
    } finally {
      if (mine === generation) {
        s.loading = false;
        if (repaint) render();
      }
    }
  }
  function chooseDate(date) {
    s.date = date;
    s.selected = null;
    s.notice = '';
    void refresh();
  }
  function chooseMonth(delta) {
    s.month = shiftMonth(s.month, delta);
    s.date =
      s.month === s.profile.today.slice(0, 7)
        ? s.profile.today
        : `${s.month}-01`;
    s.selected = null;
    s.notice = '';
    void refresh();
  }
  async function perform(recover = false) {
    if (!client || s.storageError || s.busy) return;
    s.busy = true;
    s.error = null;
    render();
    try {
      let result;
      if (recover) result = await client.recover();
      else {
        const p = s.selected;
        const answers = Object.fromEntries(
          s.profile.form.fields.map((f) => [f.key, s.answers[f.key] ?? '']),
        );
        result = await client.submit({
          offer_id: p.id,
          offer_version: p.version,
          specialist_version: p.specialist_version,
          form_version: s.profile.version,
          time_zone: p.time_zone,
          answers,
          consent: true,
        });
      }
      s.receipt = result;
      s.receiptToken = client.read().history[0].token;
      s.step = 'receipt';
      s.answers = {};
      s.selected = null;
      s.mustReview = false;
      s.notice = '';
    } catch (e) {
      s.error = e;
      if (
        [
          'booking_changed',
          'offer_full',
          'specialist_busy',
          'offer_unavailable',
          'booking_closed',
        ].includes(e.code)
      )
        s.mustReview = true;
    } finally {
      s.busy = false;
      refreshSnapshot();
      render();
      focusTitle();
    }
  }
  async function openReceipt(token) {
    if (!client || s.busy) return;
    s.busy = true;
    s.error = null;
    render();
    try {
      s.receipt = await client.receipt(token);
      s.receiptToken = token;
      s.step = 'receipt';
      client.acknowledge();
    } catch (e) {
      s.error = e;
    } finally {
      s.busy = false;
      render();
      focusTitle();
    }
  }
  async function refreshReceipt() {
    const token = s.receiptToken;
    try {
      const value = await client.receipt(token);
      if (s.step !== 'receipt' || s.receiptToken !== token || s.busy) return;
      if (value.version !== s.receipt.version || s.error) {
        s.receipt = value;
        s.error = null;
        render();
      }
    } catch (error) {
      if (
        s.step === 'receipt' &&
        s.receiptToken === token &&
        !s.busy &&
        (!s.error || s.error.code !== error.code)
      ) {
        s.error = error;
        render();
      }
    }
  }
  function calendar() {
    const first = s.profile.today.slice(0, 7),
      last = shiftMonth(first, 12);
    const specs = node(
      'select',
      {
        id: 'public-specialist',
        value: s.specialist,
        disabled: locked(),
        onChange: (e) => {
          s.specialist = e.target.value;
          s.selected = null;
          s.notice = '';
          void refresh();
        },
      },
      node('option', { value: '' }, 'Любой специалист'),
      ...s.profile.specialists.map((p) =>
        node('option', { value: p.id }, p.name),
      ),
    );
    if (
      s.specialist &&
      !s.profile.specialists.some((p) => p.id === s.specialist)
    ) {
      specs.append(
        node(
          'option',
          { value: s.specialist, disabled: true },
          'Специалист недоступен',
        ),
      );
      specs.value = s.specialist;
    }
    const picker = node(
      'aside',
      { class: 'panel calendar-panel' },
      node('h2', { tabindex: '-1' }, 'Выберите время'),
      node(
        'div',
        { class: 'field' },
        node('label', { for: 'public-specialist' }, 'Специалист'),
        specs,
      ),
      node(
        'div',
        { class: 'view-switch', role: 'group', 'aria-label': 'Вид расписания' },
        ...['calendar', 'list'].map((v) =>
          node(
            'button',
            {
              'aria-pressed': s.view === v,
              onClick: () => {
                s.view = v;
                render();
              },
            },
            v === 'calendar' ? 'Календарь' : 'Список',
          ),
        ),
      ),
      node(
        'div',
        { class: 'month-nav' },
        node(
          'button',
          {
            class: 'icon-button',
            'aria-label': 'Предыдущий месяц',
            disabled: locked() || s.month <= first,
            onClick: () => chooseMonth(-1),
          },
          '‹',
        ),
        node(
          'strong',
          {},
          new Date(`${s.month}-01T12:00:00Z`).toLocaleDateString('ru-RU', {
            month: 'long',
            year: 'numeric',
            timeZone: 'UTC',
          }),
        ),
        node(
          'button',
          {
            class: 'icon-button',
            'aria-label': 'Следующий месяц',
            disabled: locked() || s.month >= last,
            onClick: () => chooseMonth(1),
          },
          '›',
        ),
      ),
    );
    if (s.view === 'calendar') {
      const available = new Map(s.days.map((d) => [d.date, d.count]));
      picker.append(
        node(
          'div',
          { class: 'weekdays', 'aria-hidden': 'true' },
          ...['пн', 'вт', 'ср', 'чт', 'пт', 'сб', 'вс'].map((d) =>
            node('span', {}, d),
          ),
        ),
        node(
          'div',
          { class: 'month-grid', role: 'group', 'aria-label': 'Даты приёма' },
          ...monthGrid(s.month).map((date) =>
            date
              ? node(
                  'button',
                  {
                    class: 'day',
                    'aria-label': `${dateLabel(date)}${available.has(date) ? ', есть талоны' : ', талонов нет'}`,
                    'aria-pressed': s.date === date,
                    disabled: locked() || !available.has(date),
                    onClick: () => chooseDate(date),
                  },
                  Number(date.slice(8)),
                  available.has(date)
                    ? node('small', { 'aria-hidden': 'true' }, '●')
                    : null,
                )
              : node('span', { 'aria-hidden': 'true' }),
          ),
        ),
      );
    } else
      picker.append(
        node(
          'div',
          { class: 'dates-list' },
          ...s.days.map((day) =>
            node(
              'button',
              {
                class: 'date-option',
                'aria-pressed': s.date === day.date,
                disabled: locked(),
                onClick: () => chooseDate(day.date),
              },
              node('span', {}, dateLabel(day.date)),
              node('small', {}, `Талонов: ${day.count}`),
            ),
          ),
        ),
      );
    if (!s.days.length && !s.loading)
      picker.append(
        node(
          'p',
          { class: 'hint' },
          'Талонов с назначенной датой в этом месяце нет. Проверьте другие месяцы или варианты по договорённости.',
        ),
      );
    picker.append(
      node(
        'p',
        { class: 'legend' },
        `Время приёма · ${s.profile.time_zone}. Просмотр календаря не удерживает место.`,
      ),
    );
    const times = node(
      'section',
      { class: 'panel times-panel', 'aria-label': 'Доступные талоны' },
      node('h2', {}, dateLabel(s.date)),
    );
    if (s.loading && !s.items.length)
      times.append(
        node(
          'p',
          { class: 'loading', role: 'status' },
          'Проверяем доступность…',
        ),
      );
    const groups = [
      [
        'Утро',
        (i) => i.starts_local && Number(i.starts_local.slice(11, 13)) < 12,
      ],
      [
        'День',
        (i) =>
          i.starts_local &&
          Number(i.starts_local.slice(11, 13)) >= 12 &&
          Number(i.starts_local.slice(11, 13)) < 18,
      ],
      [
        'Вечер',
        (i) => i.starts_local && Number(i.starts_local.slice(11, 13)) >= 18,
      ],
      ['По договорённости', (i) => !i.starts_local],
    ];
    for (const [label, filter] of groups) {
      const items = s.items.filter(filter);
      if (!items.length) continue;
      const cards = items.map((item) =>
        node(
          'button',
          {
            class: 'slot',
            'data-focus-key': `offer-${item.id}`,
            'aria-pressed': s.selected?.id === item.id,
            disabled: locked(),
            onClick: () => {
              s.selected = item;
              s.error = null;
              s.notice = '';
              render();
            },
          },
          node('strong', {}, timeLabel(item)),
          node('span', {}, item.title || 'Приём'),
          node('small', {}, item.specialist_name),
          item.starts_at
            ? node('small', {}, offsetLabel(item.starts_at, item.time_zone))
            : null,
          !item.date ? node('small', {}, 'Дата тоже по согласованию') : null,
          item.mode === 'by_agreement'
            ? node('small', {}, `Свободных мест: ${item.remaining}`)
            : null,
        ),
      );
      times.append(
        node('h3', {}, label),
        node('div', { class: 'slots' }, ...cards),
      );
    }
    if (!s.items.length && !s.loading && !s.error) {
      times.append(
        node(
          'p',
          { class: 'empty' },
          'На выбранную дату свободных талонов нет.',
        ),
      );
      const nearest = s.days.find((d) => d.date >= s.profile.today);
      if (nearest && nearest.date !== s.date)
        times.append(
          node(
            'button',
            { class: 'secondary', onClick: () => chooseDate(nearest.date) },
            `Ближайшая дата · ${dateLabel(nearest.date)}`,
          ),
        );
      if (s.specialist)
        times.append(
          node(
            'button',
            {
              class: 'text-button',
              onClick: () => {
                s.specialist = '';
                s.selected = null;
                void refresh();
              },
            },
            'Посмотреть всех специалистов',
          ),
        );
    }
    if (s.cursor)
      times.append(
        node(
          'button',
          {
            class: 'secondary',
            disabled: s.loading || locked(),
            onClick: () => void refresh(true),
          },
          s.loading ? 'Загружаем…' : 'Ещё талоны',
        ),
      );
    times.append(
      node(
        'div',
        { class: 'actions' },
        node(
          'button',
          {
            class: 'primary',
            disabled:
              !s.selected || locked() || Boolean(s.storageError) || s.loading,
            onClick: () => {
              s.step = 'form';
              s.mustReview = false;
              s.error = null;
              render();
              focusTitle();
            },
          },
          'Продолжить',
        ),
      ),
    );
    return node('div', { class: 'booking-layout' }, picker, times);
  }
  function form() {
    const el = node('form', {
      class: 'panel booking-form',
      onSubmit: (e) => {
        e.preventDefault();
        if (!el.reportValidity() || s.mustReview || locked()) return;
        void perform();
      },
    });
    el.append(
      node(
        'div',
        { class: 'form-title' },
        node('h2', { tabindex: '-1' }, 'Проверьте запись'),
        node(
          'button',
          {
            class: 'text-button',
            disabled: locked(),
            onClick: () => {
              s.step = 'calendar';
              s.selected = null;
              s.mustReview = false;
              s.error = null;
              render();
              void load();
            },
          },
          'Изменить время',
        ),
      ),
      summary(s.selected),
    );
    const fields = node('fieldset', { disabled: locked() });
    for (const field of s.profile.form.fields) {
      const id = `answer-${field.key}`;
      const attrs = {
        id,
        name: field.key,
        required: field.required,
        value: s.answers[field.key] ?? '',
        autocomplete:
          field.kind === 'client_name'
            ? 'name'
            : field.kind === 'phone'
              ? 'tel'
              : field.kind === 'email'
                ? 'email'
                : 'off',
        onInput: (e) => {
          s.answers[field.key] = e.target.value;
        },
      };
      let input;
      if (field.kind === 'select')
        input = node(
          'select',
          attrs,
          node('option', { value: '' }, 'Выберите вариант'),
          ...field.options.map((o) => node('option', { value: o }, o)),
        );
      else if (field.kind === 'note')
        input = node('textarea', { ...attrs, rows: 3, maxlength: 1000 });
      else
        input = node('input', {
          ...attrs,
          type:
            field.kind === 'phone'
              ? 'tel'
              : field.kind === 'email'
                ? 'email'
                : 'text',
          maxlength:
            field.kind === 'client_name'
              ? 120
              : field.kind === 'phone'
                ? 64
                : field.kind === 'email'
                  ? 254
                  : 500,
        });
      fields.append(
        node(
          'div',
          { class: 'field' },
          node(
            'label',
            { for: id },
            field.label,
            field.required ? null : node('span', {}, ' · необязательно'),
          ),
          input,
        ),
      );
    }
    fields.append(
      node(
        'label',
        { class: 'consent' },
        node('input', { type: 'checkbox', name: 'consent', required: true }),
        node(
          'span',
          {},
          'Согласен(на) на обработку персональных данных в соответствии с ',
          node(
            'a',
            {
              href: s.profile.privacy_url,
              target: '_blank',
              rel: 'noopener noreferrer',
            },
            'политикой организации',
          ),
        ),
      ),
    );
    el.append(
      fields,
      node(
        'p',
        { class: 'hint' },
        `Данные получит ${s.profile.organization_name} для вашей записи. Подтверждение появится здесь; уведомления пока не отправляются.`,
      ),
      node(
        'div',
        { class: 'actions' },
        node(
          'button',
          {
            class: 'primary',
            type: 'submit',
            disabled: locked() || s.mustReview || Boolean(s.storageError),
          },
          s.busy ? 'Подтверждаем…' : 'Подтвердить запись',
        ),
      ),
    );
    return el;
  }
  function receipt() {
    const a = s.receipt;
    return node(
      'section',
      { class: 'panel receipt' },
      node(
        'div',
        { class: 'success-mark', 'aria-hidden': 'true' },
        a.status === 'confirmed' ? '✓' : '—',
      ),
      node(
        'h2',
        { tabindex: '-1' },
        a.status === 'confirmed' ? 'Вы записаны' : 'Запись отменена',
      ),
      summary(a),
      node(
        'p',
        { class: 'muted' },
        a.status === 'confirmed'
          ? `Для переноса или отмены обратитесь в организацию. ${app.dataset.parentOrigin ? 'Запись можно снова открыть в этом виджете.' : 'Запись можно снова открыть на этом устройстве.'}`
          : 'Это место освобождено. При необходимости выберите другой талон.',
      ),
      node('p', { class: 'reference' }, `Номер подтверждения · ${a.reference}`),
      node(
        'div',
        { class: 'actions' },
        node(
          'button',
          {
            class: 'secondary',
            disabled: s.busy,
            onClick: () => void openReceipt(s.receiptToken),
          },
          'Обновить запись',
        ),
        node(
          'button',
          {
            class: 'primary',
            disabled: locked(),
            onClick: () => {
              client?.acknowledge();
              s.step = 'calendar';
              s.receipt = null;
              s.answers = {};
              s.selected = null;
              s.mustReview = false;
              s.error = null;
              void load();
            },
          },
          'Записаться ещё',
        ),
      ),
    );
  }
  function render() {
    const active = root.contains(document.activeElement)
      ? document.activeElement
      : null;
    const focusId = active?.id;
    const focusKey = active?.dataset.focusKey;
    root.replaceChildren();
    refreshSnapshot();
    if (s.storageError) {
      root.append(errorBox({ code: 'storage_unavailable' }));
      if (app.dataset.parentOrigin)
        root.append(
          node(
            'p',
            { class: 'notice' },
            'Браузер не дал виджету доступ к хранилищу. До отправки анкеты откройте отдельную страницу по ссылке под виджетом. Если уже отправляли анкету, сначала уточните результат в организации — не создавайте повторную запись.',
          ),
        );
    }
    if (s.error) root.append(errorBox(s.error));
    if (s.notice)
      root.append(node('p', { class: 'notice', role: 'status' }, s.notice));
    if (s.snapshot.pending)
      root.append(
        node(
          'div',
          { class: 'notice', role: 'alert' },
          node('strong', {}, 'Результат предыдущей записи ещё не подтверждён'),
          node(
            'p',
            {},
            'Повторим исходный запрос и проверим, была ли создана запись.',
          ),
          node(
            'button',
            {
              class: 'secondary',
              disabled: s.busy || Boolean(s.storageError),
              onClick: () => void perform(true),
            },
            s.busy ? 'Проверяем…' : 'Восстановить запись',
          ),
        ),
      );
    if (!s.profile) {
      root.append(
        node(
          'p',
          { class: 'loading', role: 'status' },
          s.loading ? 'Загружаем расписание…' : 'Расписание недоступно.',
        ),
        node(
          'button',
          {
            class: 'secondary',
            disabled: s.loading,
            onClick: () => void load(),
          },
          'Попробовать снова',
        ),
      );
      return;
    }
    if (s.step === 'receipt' && s.receipt) root.append(receipt());
    else if (!s.profile.enabled)
      root.append(
        node(
          'section',
          { class: 'panel' },
          node('h2', {}, 'Онлайн-запись приостановлена'),
          node(
            'p',
            { class: 'muted' },
            'Свяжитесь с организацией, чтобы договориться о приёме.',
          ),
          node(
            'button',
            {
              class: 'secondary',
              disabled: s.busy,
              onClick: () => void load(),
            },
            'Обновить',
          ),
        ),
      );
    else if (s.step === 'form' && s.selected) root.append(form());
    else root.append(calendar());
    if (s.error && s.step !== 'form')
      root.append(
        node(
          'button',
          {
            class: 'text-button',
            disabled: s.loading || s.busy,
            onClick: () => void load(),
          },
          'Обновить расписание',
        ),
      );
    if (s.snapshot.history.length)
      root.append(
        node(
          'details',
          {
            class: 'panel saved-bookings',
            open: s.savedOpen,
            onToggle: (e) => {
              s.savedOpen = e.target.open;
            },
          },
          node('summary', {}, 'Мои записи на этом устройстве'),
          node(
            'p',
            { class: 'hint' },
            'Последние 10 записей. Контакты клиента здесь не показываются.',
            app.dataset.parentOrigin
              ? ' История относится к этому виджету; на отдельной странице она может отличаться.'
              : '',
          ),
          node(
            'div',
            { class: 'saved-list' },
            ...s.snapshot.history.map((h, i) =>
              node(
                'button',
                {
                  class: 'secondary',
                  disabled: s.busy,
                  onClick: () => void openReceipt(h.token),
                },
                `Запись ${s.snapshot.history.length - i} · ${new Date(h.created_at).toLocaleDateString('ru-RU', { timeZone: s.profile.time_zone })}`,
              ),
            ),
          ),
        ),
      );
    const target = focusId
      ? document.getElementById(focusId)
      : focusKey
        ? [...root.querySelectorAll('[data-focus-key]')].find(
            (el) => el.dataset.focusKey === focusKey,
          )
        : null;
    if (target && !target.disabled) target.focus({ preventScroll: true });
  }
  window.addEventListener('storage', (event) => {
    if (client && event.key === client.key && !s.busy) {
      refreshSnapshot();
      render();
    }
  });
  setInterval(() => {
    if (document.hidden || s.busy || s.loading || s.snapshot.pending) return;
    if (s.step === 'calendar' && !s.selected && s.profile?.enabled)
      void refresh(false, true);
    else if (s.step === 'receipt' && s.receiptToken) void refreshReceipt();
  }, 15_000);
  await load();
}
