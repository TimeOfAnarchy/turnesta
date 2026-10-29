import { useState } from 'react';
import type { FormEvent } from 'react';
import { Link } from 'react-router-dom';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api, ApiError } from './api';
import { ErrorNotice, Icon, Loading } from './components';
import { keys, useTipRecipients } from './queries';

export function QueueTips({ tenant, queue, location }: { tenant: string; queue: string; location: string }) {
  const client = useQueryClient();
  const key = [...keys.queue(tenant, queue), 'tips'];
  const settings = useQuery({ queryKey: key, queryFn: async ({ signal }) => {
    try { return await api.queueTipSettings(tenant, queue, signal); }
    catch (error) { if (error instanceof ApiError && error.code === 'tips_disabled') return null; throw error; }
  }, retry: false });
  const recipients = useTipRecipients(tenant);
  const [draft, setDraft] = useState<{ recipient: string; version: number } | null>(null);
  const mutation = useMutation({
    mutationFn: (value: { recipient: string; version: number }) => api.configureQueueTips(tenant, queue, value.recipient || null, value.version),
    retry: false, networkMode: 'always',
    onSuccess: actual => { client.setQueryData(key, actual); setDraft(null); },
    onSettled: () => client.invalidateQueries({ queryKey: key }),
  });
  if (settings.data === null) return <section className="panel queue-tips-settings"><p className="small-eyebrow">ПОСЛЕ ВИЗИТА</p><h2>Чаевые</h2><p>Тестовый режим чаевых выключен. Посетителям предложение не показывается.</p></section>;
  if (settings.isPending) return <Loading label="Загружаем настройки чаевых…"/>;
  if (!settings.data || settings.isError) return <ErrorNotice error={settings.error} retry={() => { void settings.refetch(); }}/>;
  const actual = settings.data;
  const value = draft?.recipient ?? actual.recipient_id ?? '';
  const items = [...new Map((recipients.data?.pages.flatMap(page => page.items) ?? []).filter(item => item.location_id === location).map(item => [item.id, item])).values()];
  const stale = Boolean(draft && draft.version !== actual.version);
  const disabled = mutation.isPending || settings.isFetching || stale || !navigator.onLine;
  const changed = value !== (actual.recipient_id ?? '');
  function submit(event: FormEvent) {
    event.preventDefault();
    if (!draft || disabled || !changed) return;
    mutation.mutate(draft);
  }
  return <details className="panel queue-tips-settings" aria-labelledby="queue-tips-title">
    <summary className="queue-tips-heading"><span className="queue-tips-symbol" aria-hidden="true"><Icon name="heart"/></span><div><p className="small-eyebrow">ПОСЛЕ ВИЗИТА</p><h2 id="queue-tips-title">Чаевые после обслуживания</h2></div><span className="mock-badge">{actual.recipient_id ? 'ТЕСТ · ВКЛЮЧЕНО' : 'ВЫКЛЮЧЕНО'}</span><Icon name="chevron"/></summary>
    <p>После завершения обслуживания посетитель сможет открыть страницу чаевых. Это добровольно: сейчас работает эмулятор без списания денег.</p>
    <form onSubmit={submit}>
      <div className="form-field"><label htmlFor="queue-tip-recipient">Получатель на этой точке</label><select id="queue-tip-recipient" value={value} disabled={disabled} aria-describedby="queue-tip-hint" onChange={event => { setDraft({ recipient: event.target.value, version: draft?.version ?? actual.version }); mutation.reset(); }}>
        <option value="">Не предлагать чаевые</option>
        {actual.recipient_id && !items.some(item => item.id === actual.recipient_id) && <option value={actual.recipient_id}>{actual.recipient_name}</option>}
        {items.map(item => <option key={item.id} value={item.id}>{item.display_name}</option>)}
      </select><p className="field-hint" id="queue-tip-hint">Выбранный получатель будет виден посетителю. Он задаётся для очереди и не определяется по вызвавшему сотруднику.</p></div>
      {recipients.isPending && <p className="field-hint" role="status">Загружаем получателей…</p>}
      {recipients.isError && <ErrorNotice error={recipients.error} retry={() => { void recipients.refetch(); }}/>}
      {!recipients.isPending && !recipients.isError && items.length === 0 && <p className="field-hint">На загруженных страницах нет получателей этой точки. Добавьте его в разделе «Чаевые» или загрузите следующие страницы.</p>}
      {recipients.hasNextPage && <button className="text-button" type="button" disabled={recipients.isFetching || mutation.isPending} onClick={() => { void recipients.fetchNextPage(); }}>Загрузить ещё получателей</button>}
      {stale && <div className="conflict-notice" role="alert"><p>Настройки изменились. Проверьте сохранённого получателя перед новым выбором.</p><button className="text-button" type="button" onClick={() => { setDraft(null); mutation.reset(); }}>Показать сохранённые настройки</button></div>}
      {mutation.isError && <><ErrorNotice error={mutation.error}/><p className="field-hint">Результат проверяется чтением настроек. Повторное сохранение выполняется только по нажатию кнопки.</p></>}
      {mutation.isSuccess && <p className="success-notice" role="status">{actual.recipient_id ? 'Предложение чаевых включено.' : 'Предложение чаевых выключено.'}</p>}
      <div className="queue-tips-actions"><button className="button secondary" type="submit" disabled={disabled || !changed}>{mutation.isPending ? 'Сохраняем…' : 'Сохранить'}</button><Link className="text-button" to={`/organizations/${tenant}/tips`}>Управлять получателями <Icon name="external"/></Link>
        {actual.public_slug && <a className="text-button" href={`/tips/${encodeURIComponent(actual.public_slug)}`} target="_blank" rel="noreferrer">Тестовая страница <Icon name="external"/></a>}
      </div>
    </form>
  </details>;
}
