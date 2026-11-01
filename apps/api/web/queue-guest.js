import { GuestError, GuestTicketClient, guestErrorMessage, guestRequest, guestTipLink } from '/assets/queue-client.js';
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
const serviceChoice=document.getElementById('guest-service-choice');
const serviceSelect=document.getElementById('guest-service');
const serviceDescription=document.getElementById('guest-service-description');
const serviceMore=document.getElementById('guest-service-more');
const ticketService=document.getElementById('ticket-service');
let services=[];
let nextService=null;
let selectedService=null;
let loadingServices=false;
const tipCard = document.getElementById('guest-tip');
const tipRecipient = document.getElementById('guest-tip-recipient');
const tipLink = document.getElementById('guest-tip-link');
let tipFresh = false;
let client;
let profile;
let busy = false;
let failure = null;
let initializing = true;
try { client = new GuestTicketClient(slug, window.localStorage, navigator.locks); }
catch (error) { failure = error instanceof GuestError ? error : new GuestError(0, 'storage_unavailable'); }

function render() {
  const ticket = client?.ticket;
  const pending = busy || Boolean(client?.busy) || loadingServices;
  const active = ticket && (ticket.status === 'waiting' || ticket.status === 'called');
  const recoveringService=Boolean(client?.token) && !ticket && !client?.expired && !client?.serviceRejected;
  serviceChoice.hidden=Boolean(active) || recoveringService || !profile?.join_open || !client || initializing || (!services.length && !selectedService && !client?.serviceRejected);
  serviceSelect.disabled=pending || loadingServices;
  serviceMore.disabled=pending || loadingServices;
  serviceMore.hidden=!nextService;
  ticketService.hidden=!ticket?.service_name;
  ticketService.textContent=ticket?.service_name ?? '';
  const tip = tipFresh && !failure && navigator.onLine ? guestTipLink(ticket) : null;
  tipCard.hidden = !tip;
  tipLink.removeAttribute('href');
  tipRecipient.textContent = '';
  if (tip) { tipLink.href = tip.href; tipRecipient.textContent = tip.name; }
  number.hidden = !ticket; date.hidden = !ticket; ahead.hidden = ticket?.status !== 'waiting';
  const destination = document.getElementById('ticket-destination');
  destination.hidden = ticket?.status !== 'called' || !ticket?.station_name || !tipFresh || Boolean(failure) || !navigator.onLine;
  destination.textContent = ticket?.station_name ? `Подойдите: ${ticket.station_name}` : '';
  if (ticket) {
    number.textContent = `№ ${String(ticket.number).padStart(3, '0')}`;
    const [year, month, day] = ticket.service_date.split('-'); date.textContent = `${day}.${month}.${year}`;
    if (ticket.status === 'waiting') {
      heading.textContent = 'Ваш талон'; message.textContent = 'Ожидайте вызова сотрудника.';
      ahead.textContent = `Ожидающих перед вами: ${ticket.people_ahead}. Это не оценка времени ожидания.`;
    } else if (ticket.status === 'called') {
      heading.textContent = 'Вас вызывают'; message.textContent = ticket.station_name ? 'Ваш талон принят в обслуживание.' : 'Подойдите к сотруднику точки. Ваш талон принят в обслуживание.';
    } else if (ticket.status === 'completed') {
      heading.textContent = 'Обслуживание завершено'; message.textContent = 'Спасибо за визит! При необходимости можно получить новый талон.';
    } else {
      heading.textContent = 'Талон отменён'; message.textContent = 'Если вам ещё нужна услуга, можно получить новый талон.';
    }
  } else if (client?.expired) {
    heading.textContent = 'Предыдущий визит завершён'; message.textContent = 'Срок просмотра старого талона истёк.';
  } else if (client?.serviceRejected) {
    heading.textContent='Услуга недоступна'; message.textContent='Выберите другую услугу или продолжите без уточнения.';
  } else if (client?.token) {
    heading.textContent = 'Восстанавливаем ваш талон'; message.textContent = 'Получение могло выполниться. Повтор прежнего запроса вернёт тот же талон.';
  } else {
    heading.textContent = initializing ? 'Загружаем очередь…' : profile?.join_open ? 'Получите талон' : 'Выдача приостановлена';
    message.textContent = profile?.join_open ? 'Нажмите кнопку, чтобы занять очередь без регистрации.' : 'Сотрудник точки сможет подсказать, когда возобновится выдача.';
  }
  const recovering = Boolean(client?.token) && !ticket && !client?.expired;
  join.hidden = !client || initializing || Boolean(active) || (!recovering && !profile?.join_open);
  join.disabled = pending || !navigator.onLine;
  join.textContent = pending ? 'Получаем талон…' : recovering && !client?.serviceRejected ? 'Повторить получение талона' : ticket || client?.expired ? 'Получить новый талон' : 'Получить талон';
  update.disabled = pending;
  notice.className = failure ? 'guest-notice guest-error' : 'guest-notice';
  notice.setAttribute('role', failure ? 'alert' : 'status');
  notice.textContent = failure ? guestErrorMessage(failure) : pending ? 'Ждём подтверждения сервера…'
    : active ? 'Оставьте страницу открытой — состояние обновляется автоматически.' : '';
}

async function refresh() {
  if (busy || client?.busy || loadingServices || !navigator.onLine) return false;
  busy = true; render();
  const results = await Promise.allSettled([
    guestRequest(`/api/public/queues/${encodeURIComponent(slug)}`),
    client ? client.refresh() : Promise.reject(failure),
    guestRequest(`/api/public/queues/${encodeURIComponent(slug)}/services`),
  ]);
  if (results[0].status === 'fulfilled') profile = results[0].value;
  if (results[2].status === 'fulfilled') { services=results[2].value.items;nextService=results[2].value.next_cursor;renderServices(); }
  failure = results.find((result,index) => result.status === 'rejected' && !(index===1 && (client?.expired && result.reason?.status===410 || client?.serviceRejected && result.reason?.status===404)))?.reason ?? null;
  tipFresh = !failure;
  initializing = false; busy = false; render();
  return !failure;
}

join.addEventListener('click', async () => {
  if (!client || busy || client.busy || loadingServices || !navigator.onLine) return;
  busy = true; failure = null; render();
  try { await client.join(client.canStartNew(),serviceSelect.value || null); tipFresh = true; }
  catch (error) { failure = error instanceof GuestError && error.code==='service_unavailable' ? null : error; tipFresh = false; }
  finally { busy = false; render(); }
});
function renderServices() {
  const current=serviceSelect.value;
  selectedService=services.find(item=>item.id===current) ?? selectedService;
  const options=[new Option('Без уточнения услуги','')];
  if(selectedService && !services.some(item=>item.id===selectedService.id)) options.push(new Option(`${selectedService.name} · прежний выбор`,selectedService.id));
  for(const service of services) options.push(new Option(service.name,service.id));
  serviceSelect.replaceChildren(...options); serviceSelect.value=current;
  serviceDescription.textContent=selectedService?.description ?? '';
}
serviceSelect.addEventListener('change',()=>{selectedService=services.find(item=>item.id===serviceSelect.value) ?? null;serviceDescription.textContent=selectedService?.description ?? '';});
serviceMore.addEventListener('click',async()=>{
  if(!nextService || loadingServices || busy)return;
  loadingServices=true; render();
  try {const page=await guestRequest(`/api/public/queues/${encodeURIComponent(slug)}/services?after=${encodeURIComponent(nextService)}`);services=[...new Map([...services,...page.items].map(item=>[item.id,item])).values()];nextService=page.next_cursor;renderServices();failure=null;}
  catch(error){failure=error;}
  finally{loadingServices=false;render();}
});
update.addEventListener('click', () => live.refresh());
const live = watchQueue({
  url: `/api/public/queues/${encodeURIComponent(slug)}/events?audience=guest`, refresh,
  disconnect: () => { tipFresh = false; render(); },
  status: value => {
    const badge = document.getElementById('guest-live-status'); badge.dataset.state = value; badge.textContent = liveLabels[value]; render();
  },
});
render();
