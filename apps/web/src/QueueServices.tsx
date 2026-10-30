import { useState } from 'react';
import type { FormEvent } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { api, ApiError, validateName } from './api';
import type { Service, ServiceDraft } from './api';
import { ErrorNotice, Icon, Loading, Modal } from './components';
import { keys, useQueueServices } from './queries';

function toDraft(service: Service): ServiceDraft { return { name:service.name,description:service.description,enabled:service.enabled }; }

export function QueueServices({ tenant, queue, canManage }: { tenant: string; queue: string; canManage: boolean }) {
  const services = useQueueServices(tenant, queue);
  const items = [...new Map((services.data?.pages.flatMap(page => page.items) ?? []).map(item => [item.id,item])).values()];
  const [editing, setEditing] = useState<Service | 'new' | null>(null);
  const [success, setSuccess] = useState('');
  return <><details className="panel queue-services">
    <summary><span className="queue-services-icon" aria-hidden="true"><Icon name="grid"/></span><span><span className="small-eyebrow">ЧЕМ ПОМОЧЬ ПОСЕТИТЕЛЮ</span><strong>Услуги очереди</strong></span><span className="count-badge">{items.length}{services.hasNextPage ? '+' : ''}</span><Icon name="chevron"/></summary>
    <p className="queue-services-intro">Посетитель или сотрудник может уточнить услугу при выдаче талона. Все услуги обслуживаются в общей очереди по порядку прихода.</p>
    {success && <p className="field-hint" role="status">{success}</p>}
    {services.isPending ? <Loading label="Загружаем услуги…"/> : services.isError ? <ErrorNotice error={services.error} retry={() => { void services.refetch(); }}/> : !items.length ? <div className="queue-services-empty"><h3>Пока без разделения на услуги</h3><p>Очередь уже работает. Добавьте услуги, чтобы команда знала, с чем пришёл посетитель.</p></div> : <ul className="queue-service-list">{items.map(item => <li key={item.id}><div><h3>{item.name}</h3>{item.description && <p>{item.description}</p>}<span className={item.enabled ? 'service-available' : 'subtle'}>{item.enabled ? 'Доступна для выбора' : 'Выключена'}</span></div>{canManage && <button type="button" className="text-button" onClick={() => { setSuccess(''); setEditing(item); }} aria-label={`Изменить услугу ${item.name}`}>Изменить</button>}</li>)}</ul>}
    <div className="queue-services-actions">{canManage && <button className="button secondary" type="button" disabled={services.isPending || services.isError} onClick={() => { setSuccess(''); setEditing('new'); }}><Icon name="plus"/>Добавить услугу</button>}{services.hasNextPage && <button type="button" className="text-button" onClick={() => { void services.fetchNextPage(); }} disabled={services.isFetching}>Показать ещё услуги</button>}</div>
  </details>{editing && <ServiceEditor key={editing === 'new' ? 'new' : editing.id} tenant={tenant} queue={queue} initial={editing === 'new' ? null : editing} current={editing === 'new' ? undefined : items.find(item => item.id === editing.id)} close={() => setEditing(null)} saved={() => { setSuccess('Услуга сохранена. Ранее выданные талоны сохраняют прежнее название.'); setEditing(null); }}/>}</>;
}

function ServiceEditor({ tenant, queue, initial, current, close, saved }: { tenant: string; queue: string; initial: Service | null; current?: Service; close: () => void; saved: () => void }) {
  const client = useQueryClient();
  const [draft,setDraft] = useState<ServiceDraft>(initial ? toDraft(initial) : { name:'',description:'',enabled:true });
  const [version,setVersion] = useState(initial?.version);
  const [invalid,setInvalid] = useState<string>();
  const mutation = useMutation({ mutationFn: () => initial ? api.updateService(tenant,queue,initial.id,draft,version!) : api.createService(tenant,queue,draft),
    retry:false,networkMode:'always',onSuccess:saved,onSettled:()=>client.invalidateQueries({queryKey:keys.services(tenant,queue)}) });
  const uncertain = mutation.isError && (!(mutation.error instanceof ApiError) || mutation.error.status >= 500);
  const conflict = Boolean(initial && current && current.version !== version);
  function submit(event:FormEvent) { event.preventDefault(); if(mutation.isPending || uncertain || conflict)return; const error=validateName(draft.name) || ([...draft.description.trim()].length>500 || /[\u0000-\u001f\u007f-\u009f]/u.test(draft.description) ? 'Описание — до 500 символов, без переносов строк и управляющих знаков.' : undefined);setInvalid(error);if(!error)mutation.mutate(); }
  return <Modal title={initial ? 'Изменить услугу' : 'Новая услуга'} description="Активные услуги видны посетителям по опубликованной ссылке очереди. Выключение не меняет ранее выданные талоны." close={close} busy={mutation.isPending}>
    <form onSubmit={submit} noValidate><div className="form-field"><label htmlFor="service-name">Название услуги</label><input id="service-name" autoComplete="off" value={draft.name} disabled={mutation.isPending || uncertain} onChange={e=>setDraft({...draft,name:e.target.value})}/></div>
      <div className="form-field"><label htmlFor="service-description">Краткое описание <span className="subtle">· необязательно</span></label><input id="service-description" value={draft.description} disabled={mutation.isPending || uncertain} onChange={e=>setDraft({...draft,description:e.target.value})}/><p className="field-hint">До 500 символов. Например, что входит в услугу или что взять с собой.</p></div>
      <label className="service-enabled"><input type="checkbox" checked={draft.enabled} disabled={mutation.isPending || uncertain} onChange={e=>setDraft({...draft,enabled:e.target.checked})}/>Доступна для новых талонов</label>
      {invalid && <p className="field-error" role="alert">{invalid}</p>}
      {mutation.isError && <ErrorNotice error={mutation.error}/>}
      {conflict && <div className="conflict-notice" role="alert"><p>Услугу уже изменили. Сохранённое название: {current?.name}.</p><button type="button" className="text-button" onClick={()=>{setDraft(toDraft(current!));setVersion(current!.version);mutation.reset();}}>Загрузить сохранённую версию</button></div>}
      {uncertain && <div className="conflict-notice" role="alert"><p>Услуга могла сохраниться. Закройте форму и проверьте каталог перед новой попыткой.</p><button className="text-button" type="button" onClick={close}>Закрыть и проверить каталог</button></div>}
      <div className="dialog-actions"><button className="button secondary" type="button" onClick={close} disabled={mutation.isPending}>Отмена</button><button className="button primary" type="submit" disabled={mutation.isPending || uncertain || conflict}>{mutation.isPending ? 'Сохраняем…' : 'Сохранить услугу'}</button></div>
    </form>
  </Modal>;
}
