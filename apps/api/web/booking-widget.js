(() => {
  'use strict';
  const script = document.currentScript;
  if (!script) return;
  const service = new URL(script.src).origin;
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
    const status = document.createElement('p');
    status.setAttribute('role', 'status');
    container.replaceChildren(status, fallback);
    if (
      window.parent !== window ||
      location.origin === service ||
      !crypto.randomUUID
    ) {
      status.textContent = 'Для записи откройте отдельную страницу.';
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
      status.textContent = 'Подключаем онлайн-запись…';
      timer = setTimeout(() => {
        if (!connected)
          status.textContent =
            'Не удалось загрузить виджет. Воспользуйтесь отдельной страницей записи.';
      }, 20_000);
      init();
    };
    window.addEventListener('message', message);
    frame.addEventListener('load', wait);
    const src = new URL(`/embed${url.pathname}`, service);
    src.searchParams.set('parent_origin', location.origin);
    src.searchParams.set('channel', channel);
    frame.src = src.href;
    container.insertBefore(frame, status);
    wait();
    const cleanup = () => {
      clearTimeout(timer);
      window.removeEventListener('message', message);
      frame.removeEventListener('load', wait);
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
