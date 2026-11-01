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
    mutationFn: ({ kind, enabled, version }: { kind: 'joining' | 'display'; enabled: boolean; version: number }) => kind === 'joining'
      ? api.configureQueuePublicAccess(tenant, queue, enabled, version) : api.configureQueueDisplay(tenant, queue, enabled, version),
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
  return <><section className="panel queue-public-access" aria-labelledby="queue-public-title">
    <div className="queue-public-content"><p className="small-eyebrow">ДЛЯ ПОСЕТИТЕЛЯ</p><h2 id="queue-public-title">Талон по ссылке или QR</h2>
      <p>{value.public_url ? value.join_open ? 'Посетители могут получить талон без входа и следить за своим вызовом.' : 'Выдача новых талонов приостановлена. Ранее выданные талоны остаются доступны посетителям.' : 'Подключите публичную страницу, чтобы посетители могли самостоятельно занять очередь.'}</p>
      {value.public_url && <><label className="field-label" htmlFor="queue-public-url">Публичная ссылка очереди</label><input id="queue-public-url" className="queue-public-url" readOnly value={value.public_url}/><div className="queue-command-actions"><button className="button secondary" type="button" onClick={() => { void copy(); }}>Скопировать</button><a className="button secondary" href={value.public_url} target="_blank" rel="noreferrer">Открыть страницу <Icon name="external"/></a></div>{copied && <p className="field-hint" role="status">{copied}</p>}</>}
      {canManage ? <button className="text-button queue-public-toggle" type="button" disabled={mutation.isPending || settings.isFetching} onClick={() => mutation.mutate({ kind: 'joining', enabled: !value.join_open, version: value.version })}>{mutation.isPending && mutation.variables.kind === 'joining' ? 'Сохраняем…' : value.join_open ? 'Приостановить выдачу по ссылке' : value.public_url ? 'Возобновить выдачу по ссылке' : 'Включить публичную выдачу'}</button> : <p className="field-hint">Включать и приостанавливать выдачу могут владелец и управляющие.</p>}
    </div>
    {value.qr_url && <div className="queue-public-qr"><img src={value.qr_url} width="160" height="160" alt="QR-код публичной страницы очереди"/><a className="text-button" href={value.qr_url} download="turnesta-queue.svg">Скачать QR (SVG)</a><p>Разместите на точке для посетителей.</p></div>}
  </section>
    <section className="panel queue-display-settings" aria-labelledby="queue-display-title">
      <p className="small-eyebrow">ЭКРАН НА ТОЧКЕ</p><h2 id="queue-display-title">Обезличенное табло</h2>
      <p>{value.display_enabled ? 'Табло показывает номера, даты и окна вызванных талонов, а также число ожидающих. Имена и данные сотрудников не публикуются.' : 'Включите отдельный публичный экран для посетителей. Самостоятельная выдача талонов настраивается независимо.'}</p>
      {value.display_url && <><label className="field-label" htmlFor="queue-display-url">Публичная ссылка табло</label><input id="queue-display-url" className="queue-public-url" readOnly value={value.display_url}/><p><a className="button secondary" href={value.display_url} target="_blank" rel="noreferrer">Открыть табло <Icon name="external"/></a></p></>}
      {canManage ? <button className="text-button" type="button" disabled={mutation.isPending || settings.isFetching} onClick={() => mutation.mutate({ kind: 'display', enabled: !value.display_enabled, version: value.version })}>{mutation.isPending && mutation.variables.kind === 'display' ? 'Сохраняем…' : value.display_enabled ? 'Отключить табло' : 'Включить табло'}</button> : <p className="field-hint">Публиковать и отключать табло могут владелец и управляющие.</p>}
      {mutation.isError && <><ErrorNotice error={mutation.error}/><p className="field-hint">Проверьте текущие настройки перед повторным изменением.</p></>}
    </section>
  </>;
}
