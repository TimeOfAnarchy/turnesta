(() => {
  'use strict';
  const script = document.currentScript;
  if (!script) return;
  const service = new URL(script.src).origin;
  // Host-side parts live in a shadow root, so the site's CSS and the widget's CSS
  // never meet. A constructed sheet needs no request and no style-src on the site.
  const css = `
    .turnesta { all: initial; display: block; color: #12261d;
      font: 400 15px/1.55 Inter, "Segoe UI", -apple-system, BlinkMacSystemFont, system-ui, sans-serif; }
    .card { position: relative; min-height: 96px; overflow: hidden; background: #fff;
      border: 1px solid #e2e6e2; border-radius: 16px;
      box-shadow: 0 1px 2px rgba(16, 36, 27, .04), 0 6px 18px rgba(16, 36, 27, .06); }
    iframe { transition: opacity .2s ease; }
    .status { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center;
      gap: 10px; margin: 0; padding: 20px; text-align: center; color: #5f6d65; pointer-events: none; }
    .status[hidden] { display: none; }
    .status::before { content: ""; flex: none; width: 16px; height: 16px; border-radius: 50%;
      border: 2px solid #e2e6e2; border-top-color: #1f5a41; animation: turnesta-spin .8s linear infinite; }
    .status[data-settled] { position: static; }
    .status[data-settled]::before { display: none; }
    .status:not([hidden]) ~ iframe { opacity: 0; }
    .status[data-settled] ~ iframe { height: 0 !important; }
    .fallback { position: relative; margin: 0; padding: 10px 16px; border-top: 1px solid #eef1ee;
      background: #fafbfa; font-size: 13px; line-height: 1.5; color: #5f6d65; }
    .fallback a { color: #1f5a41; font-weight: 600; text-underline-offset: 3px; }
    .fallback a:focus-visible { outline: 2px solid #2f7a57; outline-offset: 2px; border-radius: 3px; }
    .fallback small { font-size: 12px; }
    @keyframes turnesta-spin { to { transform: rotate(360deg); } }
    @media (prefers-reduced-motion: reduce) {
      iframe { transition: none; }
      .status::before { animation: none; }
    }`;
  let sheet;
  function isolated(container) {
    let root = container;
    try {
      root = container.attachShadow({ mode: 'open' });
    } catch {
      return root;
    }
    try {
      if (!sheet) {
        sheet = new CSSStyleSheet();
        sheet.replaceSync(css);
      }
      root.adoptedStyleSheets = [sheet];
    } catch {
      sheet = undefined;
    }
    return root;
  }
  function mount(container) {
    if (container.dataset.turnestaMounted) return;
    container.dataset.turnestaMounted = 'true';
    let url;
    try {
      url = new URL(container.dataset.turnestaBooking);
    } catch {
      return;
    }
    if (
      url.origin !== service ||
      !/^\/book\/[a-f0-9-]{36}$/.test(url.pathname) ||
      url.search ||
      url.hash ||
      url.username ||
      url.password
    )
      return;
    const link = document.createElement('a');
    link.href = url.href;
    link.target = '_blank';
    link.rel = 'noopener noreferrer';
    link.textContent = 'Открыть запись на отдельной странице';
    const fallback = document.createElement('p');
    fallback.append(link);
    const caution = document.createElement('small');
    caution.textContent =
      ' Уже отправили анкету? Проверьте результат в виджете перед новой записью.';
    fallback.append(caution);
    fallback.className = 'fallback';
    const status = document.createElement('p');
    status.className = 'status';
    status.setAttribute('role', 'status');
    const card = document.createElement('div');
    card.className = 'card';
    card.append(status, fallback);
    const widget = document.createElement('div');
    widget.className = 'turnesta';
    widget.append(card);
    isolated(container).replaceChildren(widget);
    if (
      window.parent !== window ||
      location.origin === service ||
      !crypto.randomUUID
    ) {
      status.textContent = 'Для записи откройте отдельную страницу.';
      status.dataset.settled = 'true';
      if (location.origin === service)
        console.warn(
          `Turnesta: виджет не показывается на адресе самого сервиса (${service}). Откройте страницу с другого адреса и добавьте его в разрешённые сайты.`,
        );
      return;
    }
    const channel = crypto.randomUUID();
    const frame = document.createElement('iframe');
    frame.title = 'Онлайн-запись';
    frame.referrerPolicy = 'no-referrer';
    frame.setAttribute(
      'sandbox',
      'allow-scripts allow-same-origin allow-forms',
    );
    frame.setAttribute(
      'allow',
      "camera 'none'; microphone 'none'; geolocation 'none'",
    );
    frame.style.width = '100%';
    frame.style.height = '720px';
    frame.style.border = '0';
    frame.style.display = 'block';
    let connected = false,
      timer;
    const init = () =>
      frame.contentWindow?.postMessage(
        { v: 1, type: 'turnesta:init', channel },
        service,
      );
    const message = (event) => {
      const d = event.data;
      if (
        event.origin !== service ||
        event.source !== frame.contentWindow ||
        !d ||
        typeof d !== 'object' ||
        Array.isArray(d) ||
        d.v !== 1 ||
        d.channel !== channel
      )
        return;
      const fields = Object.keys(d).sort().join(',');
      if (d.type === 'turnesta:ready' && fields === 'channel,type,v') {
        init();
      } else if (
        d.type === 'turnesta:resize' &&
        fields === 'channel,height,type,v' &&
        Number.isInteger(d.height) &&
        d.height > 0 &&
        d.height <= 100_000
      ) {
        connected = true;
        clearTimeout(timer);
        status.textContent = '';
        status.hidden = true;
        frame.style.height = `${Math.min(2400, Math.max(320, d.height))}px`;
      }
    };
    const wait = () => {
      connected = false;
      clearTimeout(timer);
      status.hidden = false;
      delete status.dataset.settled;
      status.textContent = 'Подключаем онлайн-запись…';
      timer = setTimeout(() => {
        if (connected) return;
        status.textContent =
          'Не удалось загрузить виджет. Воспользуйтесь отдельной страницей записи.';
        status.dataset.settled = 'true';
        console.warn(
          `Turnesta: виджет не подключился. Проверьте, что ${location.origin} есть в разрешённых сайтах точки, а CSP сайта разрешает ${service} в script-src и frame-src.`,
        );
      }, 20_000);
    };
    // Until load the frame holds a blank document of this site, not of the service.
    const loaded = () => {
      wait();
      init();
    };
    window.addEventListener('message', message);
    frame.addEventListener('load', loaded);
    const src = new URL(`/embed${url.pathname}`, service);
    src.searchParams.set('parent_origin', location.origin);
    src.searchParams.set('channel', channel);
    frame.src = src.href;
    card.insertBefore(frame, fallback);
    wait();
    const cleanup = () => {
      clearTimeout(timer);
      window.removeEventListener('message', message);
      frame.removeEventListener('load', loaded);
      observer.disconnect();
    };
    const observer = new MutationObserver(() => {
      if (!container.isConnected || !frame.isConnected) cleanup();
    });
    observer.observe(document.documentElement, {
      childList: true,
      subtree: true,
    });
    window.addEventListener('pagehide', (event) => {
      if (!event.persisted) cleanup();
    });
  }
  const scan = () =>
    document.querySelectorAll('[data-turnesta-booking]').forEach(mount);
  if (document.readyState === 'loading')
    document.addEventListener('DOMContentLoaded', scan, { once: true });
  else scan();
})();
