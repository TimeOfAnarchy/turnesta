import { useEffect, useState } from 'react';
import type { FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link, useParams, useSearchParams } from 'react-router-dom';
import { api, ApiError, errorMessage, validateName } from './api';
import type { QueueCommand, QueueCommandResult, Ticket, TicketStatus } from './api';
import { useWorkspace } from './App';
import { ErrorNotice, Icon, Loading, Modal, SuccessNotice } from './components';
import { keys, useLocations, useQueues, useQueueTickets } from './queries';
import { QueueCommands, canCloseTicket, commandError, commandLabel, ticketLabel, ticketStatusLabels } from './queueCommands';
import { QueuePublicAccess } from './QueuePublicAccess';
import { watchQueue, liveLabels } from './queueLive.mjs';
import type { LiveStatus } from './queueLive.mjs';
import './queueLive.css';

export function QueuesPage() {
  const { organization, canManage } = useWorkspace();
  const locations = useLocations(organization.id);
  const [params, setParams] = useSearchParams();
  const locationItems = locations.data?.pages.flatMap((page) => page.items) ?? [];
  const location = params.get('location') ?? locationItems[0]?.id ?? '';
  const queues = useQueues(organization.id, location);
  const items = queues.data?.pages.flatMap((page) => page.items) ?? [];
  const [adding, setAdding] = useState(false);
  const [success, setSuccess] = useState('');
  useEffect(() => { document.title = `Очереди · ${organization.name} · Turnesta`; }, [organization.name]);
  // A deep link can point to a location beyond the first page.
  useEffect(() => {
    if (location && !locationItems.some((item) => item.id === location) && locations.hasNextPage && !locations.isFetching && !locations.isError) void locations.fetchNextPage();
  }, [location, locationItems, locations.hasNextPage, locations.isFetching, locations.isError, locations.fetchNextPage]);
  const selected = locationItems.find((item) => item.id === location);
  return <>
    <div className="page-heading"><div><p className="eyebrow">ОРГАНИЗАЦИЯ / ОЧЕРЕДИ</p><h1>Всему своё время<span className="heading-dot">.</span></h1><p className="page-intro">Выберите точку и откройте рабочее место оператора.</p></div>{canManage && selected && <button type="button" className="button primary" onClick={() => { setSuccess(''); setAdding(true); }}><Icon name="plus"/>Создать очередь</button>}</div>
    {success && <SuccessNotice>{success}</SuccessNotice>}
    {locations.isPending ? <Loading label="Загружаем точки…"/> : locations.isError ? <ErrorNotice error={locations.error} retry={() => { void locations.refetch(); }}/> : !locationItems.length ? <div className="panel empty-state"><span className="large-icon"><Icon name="locations"/></span><h2>Сначала нужна точка</h2><p>Очереди создаются в местах, где ваша команда принимает посетителей.</p><Link className="button secondary" to={`/organizations/${organization.id}/locations`}>К точкам организации <Icon name="arrow"/></Link></div> : <>
      <div className="queue-location-picker"><div className="form-field"><label htmlFor="queue-location">Точка</label><select id="queue-location" value={location} onChange={(event) => { setParams({ location: event.target.value }); setSuccess(''); setAdding(false); }}>
        {!selected && <option value={location}>{locations.hasNextPage ? 'Загружаем выбранную точку…' : 'Точка недоступна'}</option>}
        {locationItems.map((point) => <option key={point.id} value={point.id}>{point.name}</option>)}
      </select></div>{locations.hasNextPage && <button type="button" className="text-button" onClick={() => { void locations.fetchNextPage(); }} disabled={locations.isFetching}>Загрузить ещё точки</button>}<span className="subtle">{selected?.time_zone.replaceAll('_', ' ')}</span></div>
      <div className="section-line"><h2>Очереди точки</h2><button type="button" className="text-button tips-refresh" onClick={() => { void queues.refetch(); }} disabled={queues.isFetching}><Icon name="refresh"/>Обновить</button></div>
      {queues.isPending ? <Loading label="Загружаем очереди…"/> : queues.isError ? <ErrorNotice error={queues.error} retry={() => { void queues.refetch(); }}/> : !items.length ? <div className="panel empty-state"><span className="large-icon"><Icon name="queue"/></span><h2>Здесь начнётся очередь</h2><p>{canManage ? 'Создайте очередь для приёма посетителей на этой точке.' : 'Владелец или управляющий сможет создать очередь для этой точки.'}</p></div> : <div className="queue-grid">{items.map((queue) => <Link key={queue.id} className="panel queue-card" to={`/organizations/${organization.id}/queues/${queue.id}`}><span className="queue-card-icon"><Icon name="queue"/></span><h3>{queue.name}</h3><p>{selected?.name ?? 'Точка организации'}</p><span className="queue-card-link">Рабочее место <Icon name="arrow"/></span></Link>)}</div>}
      {queues.hasNextPage && <div className="pagination"><button className="button secondary" type="button" onClick={() => { void queues.fetchNextPage(); }} disabled={queues.isFetching}>Показать ещё очереди</button></div>}
    </>}
    {adding && selected && <QueueEditor key={location} tenant={organization.id} location={location} locationName={selected.name} close={() => setAdding(false)} saved={(name) => { setAdding(false); setSuccess(`Очередь «${name}» создана.`); }}/>}
  </>;
}

function QueueEditor({ tenant, location, locationName, close, saved }: { tenant: string; location: string; locationName: string; close: () => void; saved: (name: string) => void }) {
  const client = useQueryClient();
  const [name, setName] = useState('');
  const [invalid, setInvalid] = useState<string>();
  const mutation = useMutation({
    mutationFn: (value: string) => api.createQueue(tenant, location, value), retry: false, networkMode: 'always',
    onSuccess: (queue) => { void client.invalidateQueries({ queryKey: keys.queues(tenant, location) }); saved(queue.name); },
  });
  const uncertain = mutation.isError && (!(mutation.error instanceof ApiError) || mutation.error.status >= 500);
  function submit(event: FormEvent) {
    event.preventDefault();
    if (mutation.isPending || uncertain) return;
    const error = validateName(name); setInvalid(error);
    if (!error) mutation.mutate(name.trim());
  }
  return <Modal title="Новая очередь" description={`Точка: ${locationName}. Название поможет команде выбрать нужное рабочее место.`} close={close} busy={mutation.isPending}>
    <form onSubmit={submit} noValidate><div className="form-field"><label htmlFor="queue-name">Название очереди</label><input id="queue-name" autoComplete="off" value={name} onChange={(event) => { setName(event.target.value); setInvalid(undefined); }} disabled={mutation.isPending || uncertain} aria-invalid={Boolean(invalid)} aria-describedby={invalid ? 'queue-name-error' : undefined}/>{invalid && <p id="queue-name-error" className="field-error">{invalid}</p>}</div>
      {uncertain ? <div className="conflict-notice" role="alert"><strong>Результат создания пока неизвестен</strong><p>Очередь могла сохраниться. Проверьте список перед новой попыткой.</p><button className="text-button" type="button" onClick={() => { void client.invalidateQueries({ queryKey: keys.queues(tenant, location) }); close(); }}>Закрыть и проверить список</button></div> : mutation.isError && <ErrorNotice error={mutation.error}/>}
      <div className="dialog-actions"><button className="button secondary" type="button" onClick={close} disabled={mutation.isPending}>Отмена</button><button className="button primary" type="submit" disabled={mutation.isPending || uncertain}>{mutation.isPending ? 'Создаём…' : 'Создать очередь'}</button></div>
    </form>
  </Modal>;
}

export function QueuePage() {
  const { queueId = '' } = useParams();
  const { organization, userId } = useWorkspace();
  return <QueueWorkplace key={`${organization.id}:${queueId}:${userId}`} tenant={organization.id} queueId={queueId}/>;
}

function commandStorage() {
  try { return window.sessionStorage; } catch { return null; }
}

function receipt(result: QueueCommandResult, command: QueueCommand): string {
  if (!result.ticket) return 'Ожидающих посетителей на момент вызова не было.';
  const label = ticketLabel(result.ticket);
  if (command.type === 'issue') return `Выдан талон ${label}.`;
  if (command.type === 'call_next') return `Вызван посетитель ${label}.`;
  if (command.type === 'complete') return `Обслуживание ${label} завершено.`;
  return `Талон ${label} отменён.`;
}

function QueueWorkplace({ tenant, queueId }: { tenant: string; queueId: string }) {
  const { role, canManage, userId } = useWorkspace();
  const canOperate = role !== 'employee';
  const client = useQueryClient();
  const queue = useQuery({ queryKey: [...keys.queue(tenant, queueId), 'details'], queryFn: ({ signal }) => api.queue(tenant, queueId, signal), retry: false });
  const locations = useLocations(tenant);
  const point = locations.data?.pages.flatMap((page) => page.items).find((item) => item.id === queue.data?.location_id);
  const [status, setStatus] = useState<TicketStatus>('waiting');
  const tickets = useQueueTickets(tenant, queueId, status);
  const mine = useQuery({ queryKey: [...keys.queue(tenant, queueId), 'mine', userId], queryFn: ({ signal }) => api.tickets(tenant, queueId, 'called', null, signal, true), enabled: canOperate, retry: false });
  const ownTicket = mine.data?.items[0];
  const [journal] = useState(() => new QueueCommands({ tenant, queue: queueId, user: userId }, commandStorage()));
  const [busy, setBusy] = useState(false);
  const [success, setSuccess] = useState('');
  const [error, setError] = useState<unknown>(null);
  const [cancel, setCancel] = useState<Ticket | null>(null);
  const [online, setOnline] = useState(navigator.onLine);
  const [liveStatus, setLiveStatus] = useState<LiveStatus>('connecting');
  useEffect(() => {
    const live = watchQueue({
      url: api.queueEventsUrl(tenant, queueId), status: setLiveStatus,
      refresh: async () => {
        try {
          await Promise.all([
            client.invalidateQueries({ queryKey: keys.queue(tenant, queueId) }, { throwOnError: true }),
            client.invalidateQueries({ queryKey: keys.locations(tenant) }, { throwOnError: true }),
            client.invalidateQueries({ queryKey: keys.organization(tenant), exact: true }, { throwOnError: true }),
          ]);
          return true;
        } catch { return false; }
      },
    });
    return () => live.dispose();
  }, [tenant, queueId, client]);
  const items = tickets.data?.pages.flatMap((page) => page.items) ?? [];
  useEffect(() => {
    document.title = `${queue.data?.name ?? 'Очередь'} · Turnesta`;
  }, [queue.data?.name]);
  useEffect(() => {
    const changed = () => setOnline(navigator.onLine);
    window.addEventListener('online', changed); window.addEventListener('offline', changed);
    return () => { window.removeEventListener('online', changed); window.removeEventListener('offline', changed); };
  }, []);
  useEffect(() => {
    if (queue.data && !point && locations.hasNextPage && !locations.isFetching && !locations.isError) void locations.fetchNextPage();
  }, [queue.data, point, locations.hasNextPage, locations.isFetching, locations.isError, locations.fetchNextPage]);

  async function refresh() { await client.invalidateQueries({ queryKey: keys.queue(tenant, queueId) }); }
  const disabled = busy || Boolean(journal.pending) || Boolean(journal.storageError) || !online || queue.isError || (canOperate && (mine.isError || mine.isPending));
  const listDisabled = disabled || tickets.isError || tickets.isPending;
  async function perform(command?: QueueCommand) {
    if (journal.busy || !online || !canOperate || (command && disabled)) return;
    const actual = command ?? journal.pending?.command;
    if (!actual) return;
    setBusy(true); setSuccess(''); setError(null);
    try {
      const result = await journal.run(command);
      setSuccess(`${command ? '' : 'Результат прежнего запроса восстановлен. '}${receipt(result, actual)}`);
    } catch (failure) { setError(failure); }
    finally { try { await refresh(); } finally { setBusy(false); } }
  }

  if (queue.isPending) return <Loading label="Открываем рабочее место…"/>;
  if (!queue.data || queue.error instanceof ApiError && [403, 404].includes(queue.error.status)) return <><Link className="queue-back" to={`/organizations/${tenant}/queues`}>← К очередям</Link><ErrorNotice error={queue.error} retry={() => { void queue.refetch(); }}/></>;
  return <>
    <Link className="queue-back" to={`/organizations/${tenant}/queues?location=${encodeURIComponent(queue.data.location_id)}`}>← К очередям точки</Link>
    <div className="page-heading queue-workplace-heading"><div><p className="eyebrow">РАБОЧЕЕ МЕСТО ОПЕРАТОРА</p><h1>{queue.data.name}<span className="heading-dot">.</span></h1><p className="page-intro">{point?.name ?? 'Очередь точки'}{point ? ` · ${point.time_zone.replaceAll('_', ' ')}` : ''}</p></div><button type="button" className="button secondary" onClick={() => { void refresh(); }} disabled={queue.isFetching || tickets.isFetching || mine.isFetching}><Icon name="refresh"/>Обновить</button></div>
    <div className="queue-update-hint"><span className="queue-live-status" data-state={liveStatus} role="status" aria-live="polite">{liveLabels[liveStatus]}</span></div>
    <QueuePublicAccess tenant={tenant} queue={queueId} canManage={canManage}/>
    {!online && <div className="conflict-notice" role="status"><strong>Нет подключения</strong><p>Действия недоступны. После восстановления связи повторите нужное действие вручную.</p></div>}
    {queue.isError && <ErrorNotice error={queue.error} retry={() => { void refresh(); }}/>}
    {canOperate && mine.isError && <ErrorNotice error={mine.error} retry={() => { void mine.refetch(); }}/>}
    {journal.storageError !== null && canOperate && <div className="error-notice" role="alert"><p>Браузеру не удалось прочитать или сохранить незавершённый запрос. Новые действия заблокированы, чтобы не потерять его результат. Проверьте доступность хранилища браузера и перезагрузите страницу; если ошибка повторится, обратитесь к администратору.</p></div>}
    {journal.pending && <div className="queue-pending" role="alert"><strong>{busy ? 'Отправляем запрос…' : 'Нужно восстановить результат действия'}</strong><p>{commandLabel(journal.pending.command)} · {new Date(journal.pending.created_at).toLocaleString('ru-RU')}. Ответ ещё не подтверждён. Повтор вернёт прежний результат, а если запрос не был выполнен — выполнит его один раз.</p>{canOperate && <button type="button" className="button secondary" onClick={() => { void perform(); }} disabled={busy || !online || Boolean(journal.storageError) || error instanceof ApiError && error.code === 'idempotency_conflict'}>Повторить прежний запрос</button>}<p className="field-hint">Другие действия в этой очереди будут доступны после получения результата. Запрос сохраняется в этой вкладке при переходах и перезагрузке страницы.</p></div>}
    {success && <SuccessNotice>{success}</SuccessNotice>}
    {error !== null && !journal.storageError && <div className="error-notice" role="alert"><p>{commandError(error) ?? errorMessage(error)}</p></div>}
    {canOperate ? <section className="panel queue-console" aria-labelledby="queue-controls-title"><div><p className="small-eyebrow">ПРИЁМ ПОСЕТИТЕЛЕЙ</p><h2 id="queue-controls-title">{ownTicket ? 'Вы обслуживаете' : 'Готовы к следующему посетителю'}</h2>{ownTicket ? <><strong className="queue-current-number">{ticketLabel(ownTicket)}</strong><p>Завершите обслуживание, прежде чем вызывать следующего.</p><div className="queue-command-actions"><button className="button primary" type="button" disabled={disabled} onClick={() => { void perform({ type: 'complete', ticket_id: ownTicket.id }); }}><Icon name="check"/>Завершить обслуживание</button><button className="button secondary" type="button" disabled={disabled} onClick={() => setCancel(ownTicket)}>Отменить талон</button></div></> : <><p>Вызов выберет первого ожидающего посетителя по порядку выдачи.</p><button className="button primary" type="button" disabled={disabled} onClick={() => { void perform({ type: 'call_next' }); }}><Icon name="arrow"/>Вызвать следующего</button></>}</div><div className="queue-issue"><span className="queue-card-icon"><Icon name="queue"/></span><h3>Новый посетитель</h3><p>Выдайте талон человеку, который пришёл на точку.</p><button type="button" className="button secondary" disabled={disabled} onClick={() => { void perform({ type: 'issue' }); }}><Icon name="plus"/>Выдать талон</button></div></section> : <div className="info-strip"><Icon name="queue"/><div><h3>Доступ для просмотра</h3><p>Выдавать и обслуживать талоны могут операторы, управляющие и владелец.</p></div></div>}
    <section className="queue-tickets" aria-labelledby="queue-tickets-title"><div className="section-line"><h2 id="queue-tickets-title">Талоны очереди</h2><span className="subtle">{tickets.isFetching ? 'Обновляем…' : 'По порядку выдачи'}</span></div>
      <div className="queue-status-filter" role="group" aria-label="Состояние талонов">{(['waiting', 'called', 'completed', 'cancelled'] as const).map((value) => <button key={value} type="button" aria-pressed={status === value} onClick={() => setStatus(value)}>{value === 'waiting' ? 'Ожидают' : value === 'called' ? 'Обслуживаются' : value === 'completed' ? 'Завершены' : 'Отменены'}</button>)}</div>
      <p className="queue-time-hint">Номер и дата — по времени точки. Время выдачи — {point ? `в ${point.time_zone}` : 'по вашему устройству'}.</p>
      {tickets.isPending ? <Loading label="Загружаем талоны…"/> : tickets.isError ? <ErrorNotice error={tickets.error} retry={() => { void tickets.refetch(); }}/> : !items.length ? <div className="panel queue-empty"><Icon name="queue"/><h3>{status === 'waiting' ? 'Ожидающих посетителей нет' : 'Талонов в этом состоянии пока нет'}</h3><p>{status === 'waiting' ? 'Здесь появятся новые талоны после выдачи.' : 'Список обновится по мере обслуживания посетителей.'}</p></div> : <div className="panel queue-ticket-list">{items.map((ticket) => <article className="queue-ticket" key={ticket.id}><div><h3>{ticketLabel(ticket)}</h3><p><time dateTime={ticket.issued_at}>{formatTicketTime(ticket.issued_at, point?.time_zone)}</time> · {ticket.status === 'called' ? ticket.called_by === userId ? 'Ваш посетитель' : 'У другого оператора' : ticketStatusLabels[ticket.status]}</p></div><span className={`queue-ticket-status queue-ticket-${ticket.status}`}>{ticketStatusLabels[ticket.status]}</span>{canCloseTicket(ticket, role, userId) && <div className="queue-ticket-actions">{ticket.status === 'called' && <button type="button" className="button secondary" disabled={listDisabled} onClick={() => { void perform({ type: 'complete', ticket_id: ticket.id }); }} aria-label={`Завершить обслуживание ${ticketLabel(ticket)}`}>Завершить</button>}<button type="button" className="text-button queue-cancel" disabled={listDisabled} onClick={() => setCancel(ticket)} aria-label={`Отменить талон ${ticketLabel(ticket)}`}>Отменить</button></div>}{ticket.status === 'called' && ticket.called_by !== userId && !canManage && <span className="readonly-label">Другой оператор</span>}</article>)}</div>}
      {tickets.hasNextPage && !tickets.isError && <div className="pagination"><button type="button" className="button secondary" onClick={() => { void tickets.fetchNextPage(); }} disabled={tickets.isFetching}>Показать ещё талоны</button></div>}
    </section>
    {cancel && <Modal title={`Отменить талон ${ticketLabel(cancel)}?`} description="Отменённый талон нельзя вернуть в очередь. При необходимости посетителю можно выдать новый." close={() => { if (!busy) setCancel(null); }} busy={busy}><div className="dialog-actions"><button className="button secondary" type="button" onClick={() => setCancel(null)} disabled={busy}>Оставить талон</button><button className="button primary" type="button" disabled={disabled} onClick={async () => { await perform({ type: 'cancel', ticket_id: cancel.id }); setCancel(null); }}>{busy ? 'Отменяем…' : 'Отменить талон'}</button></div></Modal>}
  </>;
}

function formatTicketTime(value: string, timeZone?: string) {
  try { return new Intl.DateTimeFormat('ru-RU', { dateStyle: 'short', timeStyle: 'short', timeZone }).format(new Date(value)); }
  catch { return 'Время недоступно'; }
}
