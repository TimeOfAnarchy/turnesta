import { useEffect, useId, useState } from 'react';
import type { FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api, ApiError, validateName } from './api';
import type { Location, LocationDraft } from './api';
import { useWorkspace } from './App';
import { ErrorNotice, Icon, Loading, Modal, SuccessNotice } from './components';
import { keys, useLocations } from './queries';

export function LocationsPage() {
  const { organization, canManage } = useWorkspace();
  const locations = useLocations(organization.id);
  const [editing, setEditing] = useState<Location | 'new' | null>(null);
  const [success, setSuccess] = useState('');
  const items = locations.data?.pages.flatMap((page) => page.items) ?? [];
  useEffect(() => { document.title = `Точки · ${organization.name} · Turnesta`; }, [organization.name]);

  async function reloadLocation(id: string) {
    const result = await locations.refetch();
    if (result.isError) throw result.error;
    const actual = result.data?.pages.flatMap((page) => page.items).find((item) => item.id === id);
    if (!actual) throw new ApiError(404, 'not_found');
    return actual;
  }

  return <>
    <div className="page-heading"><div><p className="eyebrow">ОРГАНИЗАЦИЯ / ТОЧКИ</p><h1>Места, где вас ждут<span className="heading-dot">.</span></h1><p className="page-intro">Все точки вашей организации — в одном пространстве.</p></div>{canManage && <button type="button" className="button primary" onClick={() => { setSuccess(''); setEditing('new'); }}><Icon name="plus"/>Добавить точку</button>}</div>
    {success && <SuccessNotice>{success}</SuccessNotice>}
    <div className="section-line"><div className="section-title"><h2>Точки организации</h2>{items.length > 0 && <span className="count-badge">{items.length}{locations.hasNextPage ? '+' : ''}</span>}</div><span className="subtle">{canManage ? 'Места и часовые пояса' : 'Доступ для просмотра'}</span></div>
    {locations.isPending ? <div className="panel"><Loading label="Загружаем точки…"/></div> : locations.isError && !items.length ? <ErrorNotice error={locations.error} retry={() => { void locations.refetch(); }}/> : items.length === 0 ? <div className="panel empty-state locations-empty">
      <span className="empty-illustration" aria-hidden="true"><span className="illustration-ring"/><Icon name="locations"/><span className="illustration-dot"/></span>
      <p className="eyebrow">У ВСЕГО ЕСТЬ СВОЁ МЕСТО</p><h2>Начнём с первой точки</h2><p>{canManage ? 'Добавьте место, где работает ваша команда: салон, ресторан, клинику или офис.' : 'В организации пока нет точек. Владелец или управляющий сможет добавить первое место.'}</p>
      {canManage && <button type="button" className="button primary" onClick={() => { setSuccess(''); setEditing('new'); }}><Icon name="plus"/>Добавить первую точку</button>}
    </div> : <div className="panel location-list"><div className="list-column-headings" aria-hidden="true"><span>НАЗВАНИЕ ТОЧКИ</span><span>ЧАСОВОЙ ПОЯС</span><span/></div>{items.map((location) => <article className="location-row" key={location.id}>
      <div className="location-identity"><span className="location-icon"><Icon name="locations"/></span><div><h3>{location.name}</h3><span className="location-meta">Точка организации</span></div></div>
      <div className="timezone-value"><Icon name="globe"/><span>{location.time_zone.replaceAll('_', ' ')}</span></div>
      {canManage ? <button type="button" className="button edit-button" onClick={() => { setSuccess(''); setEditing(location); }} aria-label={`Изменить точку ${location.name}`}><Icon name="edit"/><span>Изменить</span></button> : <span className="readonly-label">Просмотр</span>}
    </article>)}</div>}
    {locations.isError && items.length > 0 && <ErrorNotice error={locations.error} retry={() => { void locations.refetch(); }}/>}
    {locations.hasNextPage && <div className="pagination"><button className="button secondary" type="button" onClick={() => { void locations.fetchNextPage(); }} disabled={locations.isFetchingNextPage}>{locations.isFetchingNextPage ? 'Загружаем…' : 'Показать ещё точки'}</button></div>}
    <div className="info-strip"><span className="info-strip-icon"><Icon name="globe"/></span><div><h3>У каждой точки — своё время</h3><p>Часовой пояс помогает команде работать по местному времени, даже если точки находятся в разных городах.</p></div></div>
    {editing && <LocationEditor key={editing === 'new' ? 'new' : editing.id} tenant={organization.id} location={editing === 'new' ? null : editing} close={() => setEditing(null)} reloadLocation={reloadLocation} saved={(name) => { setEditing(null); setSuccess(editing === 'new' ? `Точка «${name}» добавлена.` : `Изменения точки «${name}» сохранены.`); }}/>}
  </>;
}

function LocationEditor({ tenant, location, close, saved, reloadLocation }: { tenant: string; location: Location | null; close: () => void; saved: (name: string) => void; reloadLocation: (id: string) => Promise<Location> }) {
  const client = useQueryClient();
  const [basis, setBasis] = useState(location);
  const [draft, setDraft] = useState<LocationDraft>({ name: location?.name ?? '', time_zone: location?.time_zone ?? '' });
  const [errors, setErrors] = useState<{ name?: string; time_zone?: string }>({});
  const [reloadError, setReloadError] = useState<unknown>(null);
  const [reloading, setReloading] = useState(false);
  const [reloaded, setReloaded] = useState(false);
  const [zoneQuery, setZoneQuery] = useState(draft.time_zone);
  const zoneList = useId();
  useEffect(() => { const timer = window.setTimeout(() => setZoneQuery(draft.time_zone), 200); return () => window.clearTimeout(timer); }, [draft.time_zone]);
  const zones = useQuery({ queryKey: ['time-zones', zoneQuery], queryFn: ({ signal }) => api.timeZones(zoneQuery, signal), staleTime: 300_000 });
  const mutation = useMutation({
    mutationFn: (value: LocationDraft) => basis ? api.updateLocation(tenant, basis.id, value, basis.version) : api.createLocation(tenant, value),
    onSuccess: (value) => {
      void client.invalidateQueries({ queryKey: keys.locations(tenant) });
      saved(value.name);
    },
  });
  const conflict = mutation.error instanceof ApiError && mutation.error.code === 'version_conflict';
  const busy = mutation.isPending || reloading;

  function submit(event: FormEvent) {
    event.preventDefault();
    const nextErrors = { name: validateName(draft.name), time_zone: !draft.time_zone.trim() ? 'Укажите часовой пояс точки.' : undefined };
    setErrors(nextErrors);
    if (nextErrors.name || nextErrors.time_zone) return;
    mutation.mutate({ name: draft.name.trim(), time_zone: draft.time_zone.trim() });
  }

  async function reload() {
    if (!basis) return;
    setReloading(true); setReloadError(null);
    try { setBasis(await reloadLocation(basis.id)); mutation.reset(); setReloaded(true); }
    catch (error) { setReloadError(error); }
    finally { setReloading(false); }
  }

  return <Modal title={basis ? 'Изменить точку' : 'Новая точка'} description={basis ? 'Обновите название или часовой пояс места.' : 'Начните с названия и местного часового пояса.'} close={close} busy={busy}>
    <form className="editor-form" onSubmit={submit} noValidate>
      <div className="form-field"><label htmlFor="location-name">Название точки</label><input id="location-name" name="name" autoFocus autoComplete="off" value={draft.name} onChange={(event) => { setDraft({ ...draft, name: event.target.value }); setErrors({ ...errors, name: undefined }); }} aria-invalid={Boolean(errors.name)} aria-describedby={errors.name ? 'location-name-error' : 'location-name-hint'} disabled={busy}/>{errors.name ? <p className="field-error" id="location-name-error">{errors.name}</p> : <p className="field-hint" id="location-name-hint">Например, «Салон на Патриарших» или «Офис на Лесной».</p>}</div>
      <div className="form-field"><label htmlFor="location-time-zone">Часовой пояс</label><div className="input-with-icon"><Icon name="globe"/><input id="location-time-zone" name="time_zone" list={zoneList} autoComplete="off" spellCheck={false} placeholder="Начните вводить город или регион" value={draft.time_zone} onChange={(event) => { setDraft({ ...draft, time_zone: event.target.value }); setErrors({ ...errors, time_zone: undefined }); }} aria-invalid={Boolean(errors.time_zone)} aria-describedby={errors.time_zone ? 'location-time-zone-error' : 'location-time-zone-hint'} disabled={busy}/></div><datalist id={zoneList}>{zones.data?.items.map((zone) => <option key={zone} value={zone}/>)}</datalist>{errors.time_zone ? <p className="field-error" id="location-time-zone-error">{errors.time_zone}</p> : <p className="field-hint" id="location-time-zone-hint">Например, Europe/Moscow или America/New_York.{zones.isError ? ' Подсказки сейчас недоступны; можно ввести значение вручную.' : ''}</p>}</div>
      {conflict ? <div className="conflict-notice" role="alert"><strong>Точку уже изменили</strong><p>Ваши введённые данные сохранены в форме. Загрузите текущую версию, сравните её со своим вариантом и сохраните ещё раз.</p><button type="button" className="text-button" onClick={() => { void reload(); }} disabled={busy}>{reloading ? 'Загружаем…' : 'Загрузить текущую версию'}</button></div> : mutation.isError && <ErrorNotice error={mutation.error}/>}
      {reloadError !== null && <ErrorNotice error={reloadError}/>}
      {reloaded && basis && <div className="current-values" role="status"><strong>Сейчас сохранено</strong><span>{basis.name}</span><span>{basis.time_zone}</span><p>Ваш вариант остался в полях выше. При сохранении он заменит текущие значения.</p><button type="button" className="text-button" onClick={() => { setDraft({ name: basis.name, time_zone: basis.time_zone }); setErrors({}); setReloaded(false); }}>Использовать текущие значения</button></div>}
      <div className="dialog-actions"><button className="button secondary" type="button" onClick={close} disabled={busy}>Отмена</button><button className="button primary" type="submit" disabled={busy || conflict}>{mutation.isPending ? 'Сохраняем…' : basis ? 'Сохранить изменения' : 'Добавить точку'}{!mutation.isPending && <Icon name={basis ? 'check' : 'plus'}/>}</button></div>
    </form>
  </Modal>;
}
