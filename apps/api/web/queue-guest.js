import { GuestError, GuestTicketClient, guestErrorMessage, guestRequest } from '/assets/queue-client.js';
import { watchQueue, liveLabels } from '/assets/queue-live.js';

const root = document.getElementById('guest-queue');
const slug = root.dataset.slug;
const heading = document.getElementById('ticket-heading');
const number = document.getElementById('ticket-number');
const date = document.getElementById('ticket-date');
const message = document.getElementById('ticket-message');
const ahead = document.getElementById('ticket-ahead');
const notice = document.getElementById('guest-notice');
const join = document.getElementById('join-button');
const update = document.getElementById('refresh-button');
let client;
let profile;
let busy = false;
let failure = null;
let initializing = true;
try { client = new GuestTicketClient(slug, window.localStorage, navigator.locks); }
catch (error) { failure = error instanceof GuestError ? error : new GuestError(0, 'storage_unavailable'); }

function render() {
  const ticket = client?.ticket;
  const pending = busy || Boolean(client?.busy);
  const active = ticket && (ticket.status === 'waiting' || ticket.status === 'called');
  number.hidden = !ticket; date.hidden = !ticket; ahead.hidden = ticket?.status !== 'waiting';
  if (ticket) {
    number.textContent = `№ ${String(ticket.number).padStart(3, '0')}`;
    const [year, month, day] = ticket.service_date.split('-'); date.textContent = `${day}.${month}.${year}`;
    if (ticket.status === 'waiting') {
      heading.textContent = 'Ваш талон'; message.textContent = 'Ожидайте вызова сотрудника.';
      ahead.textContent = `Ожидающих перед вами: ${ticket.people_ahead}. Это не оценка времени ожидания.`;
    } else if (ticket.status === 'called') {
      heading.textContent = 'Вас вызывают'; message.textContent = 'Подойдите к сотруднику точки. Ваш талон принят в обслуживание.';
    } else if (ticket.status === 'completed') {
      heading.textContent = 'Обслуживание завершено'; message.textContent = 'Спасибо за визит! При необходимости можно получить новый талон.';
    } else {
      heading.textContent = 'Талон отменён'; message.textContent = 'Если вам ещё нужна услуга, можно получить новый талон.';
    }
  } else if (client?.expired) {
    heading.textContent = 'Предыдущий визит завершён'; message.textContent = 'Срок просмотра старого талона истёк.';
  } else if (client?.token) {
    heading.textContent = 'Восстанавливаем ваш талон'; message.textContent = 'Получение могло выполниться. Повтор прежнего запроса вернёт тот же талон.';
  } else {
    heading.textContent = initializing ? 'Загружаем очередь…' : profile?.join_open ? 'Получите талон' : 'Выдача приостановлена';
    message.textContent = profile?.join_open ? 'Нажмите кнопку, чтобы занять очередь без регистрации.' : 'Сотрудник точки сможет подсказать, когда возобновится выдача.';
  }
  const recovering = Boolean(client?.token) && !ticket && !client?.expired;
  join.hidden = !client || initializing || Boolean(active) || (!recovering && !profile?.join_open);
  join.disabled = pending || !navigator.onLine;
  join.textContent = pending ? 'Получаем талон…' : recovering ? 'Повторить получение талона' : ticket || client?.expired ? 'Получить новый талон' : 'Получить талон';
  update.disabled = pending;
  notice.className = failure ? 'guest-notice guest-error' : 'guest-notice';
  notice.setAttribute('role', failure ? 'alert' : 'status');
  notice.textContent = failure ? guestErrorMessage(failure) : pending ? 'Ждём подтверждения сервера…'
    : active ? 'Оставьте страницу открытой — состояние обновляется автоматически.' : '';
}

async function refresh() {
  if (busy || client?.busy || !navigator.onLine) return false;
  busy = true; render();
  const results = await Promise.allSettled([
    guestRequest(`/api/public/queues/${encodeURIComponent(slug)}`),
    client ? client.refresh() : Promise.reject(failure),
  ]);
  if (results[0].status === 'fulfilled') profile = results[0].value;
  failure = results.find((result) => result.status === 'rejected')?.reason ?? null;
  initializing = false; busy = false; render();
  return !failure;
}

join.addEventListener('click', async () => {
  if (!client || busy || client.busy || !navigator.onLine) return;
  busy = true; failure = null; render();
  try { await client.join(client.canStartNew()); }
  catch (error) { failure = error; }
  finally { busy = false; render(); }
});
update.addEventListener('click', () => live.refresh());
const live = watchQueue({
  url: `/api/public/queues/${encodeURIComponent(slug)}/events?audience=guest`, refresh,
  status: value => {
    const badge = document.getElementById('guest-live-status'); badge.dataset.state = value; badge.textContent = liveLabels[value]; render();
  },
});
render();
