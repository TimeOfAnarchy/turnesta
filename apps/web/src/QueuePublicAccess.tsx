import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from './api';
import { ErrorNotice, Icon } from './components';
import { keys } from './queries';

export function QueuePublicAccess({ tenant, queue, canManage }: { tenant: string; queue: string; canManage: boolean }) {
  const client = useQueryClient();
  const key = [...keys.queue(tenant, queue), 'public-access'];
  const settings = useQuery({ queryKey: key, queryFn: ({ signal }) => api.queuePublicAccess(tenant, queue, signal), retry: false });
  const [copied, setCopied] = useState('');
  const mutation = useMutation({
    mutationFn: ({ open, version }: { open: boolean; version: number }) => api.configureQueuePublicAccess(tenant, queue, open, version),
    retry: false, networkMode: 'always',
    onSuccess: (actual) => { client.setQueryData(key, actual); },
    onSettled: () => client.invalidateQueries({ queryKey: key }),
  });
  const value = settings.data;
  if (settings.isPending) return <p className="queue-update-hint" role="status">Загружаем публичную выдачу…</p>;
  if (settings.isError || !value) return <ErrorNotice error={settings.error} retry={() => { void settings.refetch(); }}/>;
  async function copy() {
    if (!value?.public_url) return;
    try { await navigator.clipboard.writeText(value.public_url); setCopied('Ссылка скопирована.'); }
    catch { setCopied('Выделите и скопируйте ссылку из поля.'); }
  }
  return <section className="panel queue-public-access" aria-labelledby="queue-public-title">
    <div className="queue-public-content"><p className="small-eyebrow">ДЛЯ ПОСЕТИТЕЛЯ</p><h2 id="queue-public-title">Талон по ссылке или QR</h2>
      <p>{value.public_url ? value.join_open ? 'Посетители могут получить талон без входа и следить за своим вызовом.' : 'Выдача новых талонов приостановлена. Ранее выданные талоны остаются доступны посетителям.' : 'Подключите публичную страницу, чтобы посетители могли самостоятельно занять очередь.'}</p>
      {value.public_url && <><label className="field-label" htmlFor="queue-public-url">Публичная ссылка очереди</label><input id="queue-public-url" className="queue-public-url" readOnly value={value.public_url}/><div className="queue-command-actions"><button className="button secondary" type="button" onClick={() => { void copy(); }}>Скопировать</button><a className="button secondary" href={value.public_url} target="_blank" rel="noreferrer">Открыть страницу <Icon name="external"/></a></div>{copied && <p className="field-hint" role="status">{copied}</p>}</>}
      {canManage ? <button className="text-button queue-public-toggle" type="button" disabled={mutation.isPending || settings.isFetching} onClick={() => mutation.mutate({ open: !value.join_open, version: value.version })}>{mutation.isPending ? 'Сохраняем…' : value.join_open ? 'Приостановить выдачу по ссылке' : value.public_url ? 'Возобновить выдачу по ссылке' : 'Включить публичную выдачу'}</button> : <p className="field-hint">Включать и приостанавливать выдачу могут владелец и управляющие.</p>}
      {mutation.isError && <><ErrorNotice error={mutation.error}/><p className="field-hint">Проверьте текущий режим выше перед повторным изменением.</p></>}
    </div>
    {value.qr_url && <div className="queue-public-qr"><img src={value.qr_url} width="160" height="160" alt="QR-код публичной страницы очереди"/><a className="text-button" href={value.qr_url} download="turnesta-queue.svg">Скачать QR (SVG)</a><p>Разместите на точке для посетителей.</p></div>}
  </section>;
}
