import { validWidgetMessage } from './booking-widget-protocol.js';

export function connectBookingEmbed(app) {
  const origin = app.dataset.parentOrigin;
  if (!origin) return Promise.resolve(true);
  document.documentElement.classList.add('booking-embedded');
  return new Promise((resolve) => {
    const channel = app.dataset.channel;
    let connected = false,
      queued = false,
      lastHeight = 0;
    const send = (type, extra = {}) =>
      parent.postMessage({ v: 1, type, channel, ...extra }, origin);
    const report = () => {
      const height = Math.ceil(app.getBoundingClientRect().height);
      if (height !== lastHeight && height > 0 && height <= 100_000) {
        lastHeight = height;
        send('turnesta:resize', { height });
      }
    };
    const measure = () => {
      if (queued || !connected) return;
      queued = true;
      requestAnimationFrame(() => {
        queued = false;
        report();
      });
    };
    const listener = (event) => {
      if (
        event.source !== parent ||
        event.origin !== origin ||
        !validWidgetMessage(event.data, channel, 'turnesta:init')
      )
        return;
      if (!connected) {
        connected = true;
        clearTimeout(deadline);
        clearInterval(announce);
        observer.observe(app);
        resolve(true);
      }
      lastHeight = 0;
      // Browsers stop animation frames for an off-screen cross-origin frame, so the
      // first size cannot wait for one: a widget below the fold must still connect.
      report();
    };
    const fail = () => {
      clearInterval(announce);
      window.removeEventListener('message', listener);
      const p = document.createElement('p');
      p.className = 'notice';
      p.textContent =
        'Не удалось подключить запись. Откройте отдельную страницу по ссылке под виджетом.';
      app.querySelector('#booking-root').replaceChildren(p);
      resolve(false);
    };
    const observer = new ResizeObserver(measure);
    window.addEventListener('message', listener);
    const announce = setInterval(() => send('turnesta:ready'), 1000);
    const deadline = setTimeout(fail, 15_000);
    window.addEventListener('pagehide', (event) => {
      if (event.persisted) return;
      clearTimeout(deadline);
      clearInterval(announce);
      observer.disconnect();
      window.removeEventListener('message', listener);
    });
    if (parent === window) {
      fail();
      clearTimeout(deadline);
    } else send('turnesta:ready');
  });
}
