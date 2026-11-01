import { useState } from 'react';
import type { FormEvent } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { api, ApiError, validateName } from './api';
import type { Station, StationDraft } from './api';
import { ErrorNotice, Icon, Loading, Modal } from './components';
import { keys, useQueueStations } from './queries';

function toDraft(station: Station): StationDraft { return { name: station.name, enabled: station.enabled }; }

export function QueueStations({ tenant, queue, canManage }: { tenant: string; queue: string; canManage: boolean }) {
  const stations = useQueueStations(tenant, queue);
  const items = [...new Map((stations.data?.pages.flatMap(page => page.items) ?? []).map(item => [item.id, item])).values()];
  const [editing, setEditing] = useState<Station | 'new' | null>(null);
  const [success, setSuccess] = useState('');
  return <><details className="panel queue-services">
    <summary><span className="queue-services-icon" aria-hidden="true"><Icon name="locations"/></span><span><span className="small-eyebrow">КУДА ПОДОЙТИ ПОСЕТИТЕЛЮ</span><strong>Окна обслуживания</strong></span><span className="count-badge">{items.length}{stations.hasNextPage ? '+' : ''}</span><Icon name="chevron"/></summary>
    <p className="queue-services-intro">Оператор выбирает окно при вызове. Посетитель видит его название в талоне и на табло. Одновременно в окне обслуживается один посетитель.</p>
    {success && <p className="field-hint" role="status">{success}</p>}
    {stations.isPending ? <Loading label="Загружаем окна…"/> : stations.isError ? <ErrorNotice error={stations.error} retry={() => { void stations.refetch(); }}/> : !items.length ? <div className="queue-services-empty"><h3>Вызовы без указания окна</h3><p>Добавьте окна или стойки, чтобы посетители понимали, куда подойти после вызова.</p></div> : <ul className="queue-service-list">{items.map(item => <li key={item.id}><div><h3>{item.name}</h3><span className={item.enabled ? 'service-available' : 'subtle'}>{!item.enabled ? `Выключено для новых вызовов${item.busy ? ' · занято' : ''}` : item.busy ? 'Занято' : 'Свободно'}</span></div>{canManage && <button type="button" className="text-button" onClick={() => { setSuccess(''); setEditing(item); }} aria-label={`Изменить окно ${item.name}`}>Изменить</button>}</li>)}</ul>}
    <div className="queue-services-actions">{canManage && <button className="button secondary" type="button" disabled={stations.isPending || stations.isError} onClick={() => { setSuccess(''); setEditing('new'); }}><Icon name="plus"/>Добавить окно</button>}{stations.hasNextPage && <button type="button" className="text-button" onClick={() => { void stations.fetchNextPage(); }} disabled={stations.isFetching}>Показать ещё окна</button>}</div>
  </details>{editing && <StationEditor key={editing === 'new' ? 'new' : editing.id} tenant={tenant} queue={queue} initial={editing === 'new' ? null : editing} current={editing === 'new' ? undefined : items.find(item => item.id === editing.id)} close={() => setEditing(null)} saved={() => { setSuccess('Окно сохранено. Уже вызванные талоны сохраняют прежнее название.'); setEditing(null); }}/>}</>;
}

function StationEditor({ tenant, queue, initial, current, close, saved }: { tenant: string; queue: string; initial: Station | null; current?: Station; close: () => void; saved: () => void }) {
  const client = useQueryClient();
  const [draft, setDraft] = useState<StationDraft>(initial ? toDraft(initial) : { name: '', enabled: true });
  const [version, setVersion] = useState(initial?.version);
  const [invalid, setInvalid] = useState<string>();
  const mutation = useMutation({ mutationFn: () => initial ? api.updateStation(tenant, queue, initial.id, draft, version!) : api.createStation(tenant, queue, draft),
    retry: false, networkMode: 'always', onSuccess: saved, onSettled: () => client.invalidateQueries({ queryKey: keys.stations(tenant, queue) }) });
  const uncertain = mutation.isError && (!(mutation.error instanceof ApiError) || mutation.error.status >= 500);
  const conflict = Boolean(initial && current && current.version !== version);
  function submit(event: FormEvent) {
    event.preventDefault();
    if (mutation.isPending || uncertain || conflict) return;
    const error = validateName(draft.name); setInvalid(error); if (!error) mutation.mutate();
  }
  return <Modal title={initial ? 'Изменить окно' : 'Новое окно'} description="Название видно посетителю и на публичном табло. Используйте обозначение места без персональных данных. Выключение не прерывает начатое обслуживание." close={close} busy={mutation.isPending}>
    <form onSubmit={submit} noValidate><div className="form-field"><label htmlFor="station-name">Название окна</label><input id="station-name" aria-invalid={Boolean(invalid)} aria-describedby={invalid ? 'station-name-error' : undefined} autoComplete="off" value={draft.name} disabled={mutation.isPending || uncertain} onChange={e => setDraft({ ...draft, name: e.target.value })}/></div>
      <label className="service-enabled"><input type="checkbox" checked={draft.enabled} disabled={mutation.isPending || uncertain} onChange={e => setDraft({ ...draft, enabled: e.target.checked })}/>Доступно для новых вызовов</label>
      {invalid && <p id="station-name-error" className="field-error" role="alert">{invalid}</p>}
      {mutation.isError && <ErrorNotice error={mutation.error}/>}
      {conflict && <div className="conflict-notice" role="alert"><p>Окно уже изменили. Сохранённое название: {current?.name}.</p><button type="button" className="text-button" onClick={() => { setDraft(toDraft(current!)); setVersion(current!.version); mutation.reset(); }}>Загрузить сохранённую версию</button></div>}
      {uncertain && <div className="conflict-notice" role="alert"><p>Окно могло сохраниться. Закройте форму и проверьте список перед новой попыткой.</p><button className="text-button" type="button" onClick={close}>Закрыть и проверить список</button></div>}
      <div className="dialog-actions"><button className="button secondary" type="button" onClick={close} disabled={mutation.isPending}>Отмена</button><button className="button primary" type="submit" disabled={mutation.isPending || uncertain || conflict}>{mutation.isPending ? 'Сохраняем…' : 'Сохранить окно'}</button></div>
    </form>
  </Modal>;
}
