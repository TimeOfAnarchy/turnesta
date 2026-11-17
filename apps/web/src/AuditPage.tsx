import { useEffect, useState } from 'react';
import type { FormEvent } from 'react';
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useWorkspace } from './App';
import { ApiError } from './api';
import { ErrorNotice, Icon, Loading, SuccessNotice } from './components';
import { auditApi } from './audit';
import type { AuditCursor, AuditEntry, AuditFilters, AuditSettings } from './audit';
import { teamApi } from './team';

export function AuditPage() {
  const { organization, role } = useWorkspace();
  useEffect(() => { document.title = `Журнал доступа · ${organization.name} · Turnesta`; }, [organization.name]);
  if (role !== 'owner') return <div className="panel empty-state"><h1>Журнал доступен владельцу</h1><p>Обратитесь к владельцу организации.</p></div>;
  return <Audit key={organization.id} tenant={organization.id}/>;
}

function Audit({ tenant }: { tenant: string }) {
  const [draft, setDraft] = useState<AuditFilters>({ from: '', to: '', actor: '' });
  const [filters, setFilters] = useState<AuditFilters>(draft);
  const [filterError, setFilterError] = useState('');
  const [settingsSaved, setSettingsSaved] = useState(false);
  const entries = useInfiniteQuery({
    queryKey: ['organization', tenant, 'audit', filters],
    initialPageParam: null as AuditCursor | null,
    queryFn: ({ pageParam, signal }) => auditApi.list(tenant, filters, pageParam, signal),
    getNextPageParam: page => page.next_cursor ?? undefined,
    retry: false,
  });
  const members = useInfiniteQuery({
    queryKey: ['organization', tenant, 'team', 'audit-filter'],
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) => teamApi.members(tenant, pageParam, signal),
    getNextPageParam: page => page.next_cursor ?? undefined,
    retry: false,
  });
  const settings = useQuery({ queryKey: ['organization', tenant, 'audit', 'settings'], queryFn: ({ signal }) => auditApi.settings(tenant, signal), retry: false });
  const people = members.data?.pages.flatMap(page => page.items) ?? [];
  const items = entries.data?.pages.flatMap(page => page.items) ?? [];
  function apply(event: FormEvent) {
    event.preventDefault();
    if (draft.from && draft.to && draft.from > draft.to) { setFilterError('Начальная дата должна быть раньше конечной.'); return; }
    setFilterError(''); setFilters({ ...draft });
  }
  return <>
    <div className="page-heading"><div><p className="eyebrow">ОРГАНИЗАЦИЯ / ДОСТУП К ДАННЫМ</p><h1>Журнал доступа<span className="heading-dot">.</span></h1><p className="page-intro">Кто и когда обращался к клиентским записям.</p></div><span className="heading-decoration" aria-hidden="true"><Icon name="users"/></span></div>
    <section className="panel audit-panel" aria-labelledby="audit-filter-heading">
      <h2 id="audit-filter-heading">Фильтры</h2>
      <form onSubmit={apply} className="audit-filters">
        <div className="form-field"><label htmlFor="audit-from">С даты</label><input id="audit-from" type="date" value={draft.from} onChange={event => setDraft({ ...draft, from: event.target.value })}/></div>
        <div className="form-field"><label htmlFor="audit-to">По дату</label><input id="audit-to" type="date" value={draft.to} onChange={event => setDraft({ ...draft, to: event.target.value })}/></div>
        <div className="form-field"><label htmlFor="audit-actor">Сотрудник</label><select id="audit-actor" value={draft.actor} onChange={event => setDraft({ ...draft, actor: event.target.value })}><option value="">Все сотрудники</option>{people.map(person => <option key={person.user_id} value={person.user_id}>{person.display_name}{person.active ? '' : ' · доступ отключён'}</option>)}</select></div>
        <button type="submit" className="button secondary">Показать</button>
      </form>
      {filterError && <p className="field-error" role="alert">{filterError}</p>}
      {members.isError && <ErrorNotice error={members.error} retry={() => { void members.refetch(); }}/>}
      {members.hasNextPage && <button type="button" className="text-button" disabled={members.isFetchingNextPage} onClick={() => { void members.fetchNextPage(); }}>Загрузить ещё сотрудников</button>}
      <p className="field-hint">Даты фильтра относятся к UTC. Запись о списке включает талоны без даты. Контакты и ответы в журнале не сохраняются.</p>
    </section>
    <section className="panel audit-panel" aria-labelledby="audit-results-heading">
      <div className="audit-heading"><h2 id="audit-results-heading">События доступа</h2><button type="button" className="button secondary" disabled={entries.isFetching} onClick={() => { void entries.refetch(); }}>Обновить</button></div>
      {entries.isPending ? <Loading label="Загружаем журнал…"/> : entries.isError && !items.length ? <ErrorNotice error={entries.error} retry={() => { void entries.refetch(); }}/> : items.length === 0 ? <p className="field-hint">Событий за выбранный период нет.</p> : <div className="audit-list">{items.map(item => <AuditRow key={item.id} item={item}/>)}</div>}
      {entries.isError && items.length > 0 && <ErrorNotice error={entries.error} retry={() => { void entries.refetch(); }}/>}
      {entries.hasNextPage && <div className="pagination"><button type="button" className="button secondary" disabled={entries.isFetchingNextPage} onClick={() => { void entries.fetchNextPage(); }}>Показать ещё</button></div>}
    </section>
    <section className="panel audit-panel" aria-labelledby="audit-retention-heading"><h2 id="audit-retention-heading">Срок хранения журнала</h2>
      {settings.isPending ? <Loading label="Загружаем настройки…"/> : settings.data ? <Retention key={`${tenant}:${settings.data.version}`} tenant={tenant} initial={settings.data} onChange={() => setSettingsSaved(false)} onSaved={() => setSettingsSaved(true)}/> : <ErrorNotice error={settings.error} retry={() => { void settings.refetch(); }}/>}
      {settingsSaved && <SuccessNotice>Срок хранения обновлён.</SuccessNotice>}
    </section>
  </>;
}

function AuditRow({ item }: { item: AuditEntry }) {
  const action = item.action === 'list' ? 'Список записей' : item.action === 'detail' ? 'Карточка клиента' : 'Результат команды';
  return <article className="audit-entry"><div><strong>{item.actor_name}</strong><span>{item.location_name}</span></div><div><strong>{action}</strong><span>{item.action === 'list' ? `Дата: ${item.visit_date}; возвращено: ${item.result_count}${item.include_cancelled ? '; с отменёнными' : ''}` : `Запись: ${item.appointment_id}`}</span></div><time dateTime={item.created_at}>{new Date(item.created_at).toLocaleString('ru-RU')}</time></article>;
}

function Retention({ tenant, initial, onChange, onSaved }: { tenant: string; initial: AuditSettings; onChange: () => void; onSaved: () => void }) {
  const client = useQueryClient();
  const [days, setDays] = useState(String(initial.retention_days));
  const key = ['organization', tenant, 'audit', 'settings'];
  const mutation = useMutation({ mutationFn: () => auditApi.configure(tenant, Number(days), initial.version), onSuccess: value => { client.setQueryData(key, value); onSaved(); } });
  const valid = /^\d+$/.test(days) && Number(days) >= 30 && Number(days) <= 3650;
  const conflict = mutation.error instanceof ApiError && mutation.error.code === 'version_conflict';
  return <form onSubmit={event => { event.preventDefault(); if (valid) { onChange(); mutation.mutate(); } }} className="audit-retention-form">
    <div className="form-field"><label htmlFor="audit-retention">Хранить события, дней</label><input id="audit-retention" type="number" min="30" max="3650" required value={days} onChange={event => { setDays(event.target.value); onChange(); }}/><p className="field-hint">От 30 до 3650 дней. По умолчанию — 365. Увеличение срока не вернёт уже удалённые события.</p></div>
    {mutation.isError && <ErrorNotice error={mutation.error}/>}
    {conflict && <button type="button" className="text-button" onClick={() => { void client.invalidateQueries({ queryKey: key }); }}>Загрузить текущую настройку</button>}
    <button type="submit" className="button primary" disabled={!valid || Number(days) === initial.retention_days || mutation.isPending || conflict}>Сохранить</button>
  </form>;
}
