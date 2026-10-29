import { DisplayClient, displayErrorMessage } from '/assets/queue-display-client.js';
import { watchQueue, liveLabels } from '/assets/queue-live.js';

const client = new DisplayClient(document.getElementById('queue-display').dataset.slug);
const calls = document.getElementById('display-calls');
const message = document.getElementById('display-message');
const connection = document.getElementById('display-connection');
const updated = document.getElementById('display-updated');
const waiting = document.getElementById('display-waiting');
const pagination = document.getElementById('display-pagination');
const joining = document.getElementById('display-joining');
const refreshButton = document.getElementById('display-refresh');
const fullscreen = document.getElementById('fullscreen');
let pageIndex = 0;
let shown = false;
let liveState = 'connecting';

function render() {
  const snapshot = client.view();
  shown = Boolean(snapshot);
  refreshButton.disabled = client.busy;
  calls.replaceChildren(); calls.hidden = !snapshot || snapshot.called.length === 0;
  message.hidden = Boolean(snapshot?.called.length);
  connection.dataset.state = liveState;
  connection.textContent = liveLabels[liveState];
  if (!snapshot) {
    waiting.textContent = '—'; pagination.textContent = ''; joining.textContent = 'Номер и дата вместе обозначают талон.';
    message.textContent = client.error ? displayErrorMessage(client.error) : 'Загружаем состояние очереди…';
    updated.textContent = client.busy ? 'Обновляем данные…' : 'Актуальность вызовов не подтверждена';
    return;
  }
  document.getElementById('display-name').textContent = snapshot.queue_name;
  document.getElementById('display-location').textContent = `${snapshot.organization_name} · ${snapshot.location_name}`;
  const time = new Date(snapshot.observed_at).toLocaleTimeString('ru-RU', { timeZone: snapshot.time_zone, hour: '2-digit', minute: '2-digit', second: '2-digit' });
  updated.textContent = `Данные на ${time} · ${snapshot.time_zone}`;
  waiting.textContent = String(snapshot.waiting_count);
  joining.textContent = snapshot.join_open ? 'Номер и дата вместе обозначают талон. Ожидайте своего вызова.' : 'Самостоятельная выдача талонов приостановлена. Обратитесь к сотруднику.';
  message.textContent = 'Сейчас вызванных посетителей нет. Ожидайте своего номера.';
  const pages = Math.max(1, Math.ceil(snapshot.called.length / 6)); pageIndex %= pages;
  pagination.textContent = pages > 1 ? `${pageIndex + 1} / ${pages} · смена каждые 10 секунд` : '';
  for (const ticket of snapshot.called.slice(pageIndex * 6, pageIndex * 6 + 6)) {
    const card = document.createElement('div'); card.className = 'display-ticket';
    const number = document.createElement('strong'); number.textContent = `№ ${String(ticket.number).padStart(3, '0')}`;
    const date = document.createElement('span'); const [year, month, day] = ticket.service_date.split('-'); date.textContent = `${day}.${month}.${year}`;
    card.append(number, date); calls.append(card);
  }
}
async function refresh() {
  if (document.hidden || client.busy || !navigator.onLine) return false;
  const task = client.refresh(); render(); await task; render(); return Boolean(client.view());
}
refreshButton.addEventListener('click', () => { live.refresh(); });
fullscreen.hidden = !document.fullscreenEnabled;
fullscreen.addEventListener('click', async () => {
  try { if (document.fullscreenElement) await document.exitFullscreen(); else await document.documentElement.requestFullscreen(); }
  catch { updated.textContent = 'Не удалось развернуть табло. Используйте полноэкранный режим браузера.'; }
});
document.addEventListener('fullscreenchange', () => { fullscreen.textContent = document.fullscreenElement ? 'Выйти из полного экрана' : 'На весь экран'; });
setInterval(() => { if (shown && !client.view()) render(); }, 1_000);
setInterval(() => { if (!document.hidden && client.view()) { pageIndex++; render(); } }, 10_000);
if (!navigator.onLine) client.invalidate('offline');
const live = watchQueue({
  url: `/api/public/queues/${encodeURIComponent(document.getElementById('queue-display').dataset.slug)}/events?audience=display`,
  refresh,
  status: value => { liveState = value; render(); },
  disconnect: () => { client.invalidate(navigator.onLine ? 'stale' : 'offline'); render(); },
  confirm: () => client.confirmFreshness(),
});
render();
