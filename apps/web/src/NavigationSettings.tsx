import { useEffect, useState } from 'react';
import type { FormEvent } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { api, ApiError } from './api';
import type { Organization, WorkspaceService } from './api';
import { ErrorNotice, Icon, SuccessNotice } from './components';
import { keys } from './queries';
import { sameServices, serviceOptions } from './navigation';

export function NavigationSettings({ organization, canManage }: { organization: Organization; canManage: boolean }) {
  const client = useQueryClient();
  const [basis, setBasis] = useState(organization);
  const [selected, setSelected] = useState<WorkspaceService[]>(organization.visible_services);
  const [saved, setSaved] = useState(false);
  const [reloading, setReloading] = useState(false);
  const [reloadError, setReloadError] = useState<unknown>(null);
  const [reloaded, setReloaded] = useState(false);
  const mutation = useMutation({
    onMutate: () => client.cancelQueries({ queryKey: keys.organization(organization.id), exact: true }),
    mutationFn: () => api.setNavigation(organization.id, selected, basis.version),
    onSuccess: value => {
      setBasis(value); setSelected(value.visible_services); setSaved(true); setReloaded(false);
      client.setQueryData(keys.organization(organization.id), value);
    },
  });
  const dirty = !sameServices(selected, basis.visible_services);
  const stale = organization.version !== basis.version || (mutation.error instanceof ApiError && mutation.error.status === 409);
  const busy = mutation.isPending || reloading;
  useEffect(() => {
    if (!dirty && !mutation.isPending && organization.version !== basis.version) {
      setBasis(organization); setSelected(organization.visible_services); setSaved(false);
    }
  }, [organization, basis.version, dirty, mutation.isPending]);

  function submit(event: FormEvent) {
    event.preventDefault();
    if (!busy && dirty && !stale && !mutation.isError) { setSaved(false); mutation.mutate(); }
  }
  async function reload() {
    setReloading(true); setReloadError(null);
    try {
      const actual = await client.fetchQuery({ queryKey: keys.organization(organization.id), queryFn: ({ signal }) => api.organization(organization.id, signal), staleTime: 0 });
      setBasis(actual); mutation.reset(); setSaved(false); setReloaded(true);
    } catch (error) { setReloadError(error); }
    finally { setReloading(false); }
  }
  const values = canManage ? selected : organization.visible_services;
  return <section className="panel settings-panel" aria-labelledby="navigation-heading">
    <div className="panel-heading"><span className="square-icon"><Icon name="grid"/></span><div><h2 id="navigation-heading">Сервисы в меню</h2><p>Оставьте разделы, с которыми работает ваша команда.</p></div></div>
    <form className="navigation-form" onSubmit={submit}>
      <fieldset className="service-options" disabled={!canManage || busy}>
        <legend className="visually-hidden">Показывать в меню организации</legend>
        {serviceOptions.map(service => <label key={service.id} className={`service-option${values.includes(service.id) ? ' selected' : ''}`}>
          <span className="square-icon"><Icon name={service.icon}/></span>
          <span className="service-option-copy"><strong>{service.label}</strong><span>{service.description}</span></span>
          <input type="checkbox" checked={values.includes(service.id)} aria-label={`Показывать «${service.label}»`} onChange={event => {
            const next = event.target.checked ? [...selected, service.id] : selected.filter(id => id !== service.id);
            setSelected(serviceOptions.filter(option => next.includes(option.id)).map(option => option.id)); setSaved(false); setReloaded(false);
          }}/>
        </label>)}
      </fieldset>
      <p className="field-hint">Выбор действует для всей организации на всех устройствах. Скрытие из меню сохраняет данные, открытые страницы и публичные ссылки.</p>
      {values.length === 0 && <p className="field-hint">В меню останутся точки, настройки и доступные вам разделы управления.</p>}
      {(stale || mutation.isError) && <div className="conflict-notice" role="alert"><strong>{stale ? 'Настройки уже изменились' : 'Сохранение не подтверждено'}</strong><p>Ваш выбор сохранён в форме. Загрузите текущие настройки перед новым сохранением.</p><button className="text-button" type="button" disabled={busy} onClick={() => { void reload(); }}>Загрузить текущие настройки</button></div>}
      {mutation.isError && !stale && <ErrorNotice error={mutation.error}/>}
      {reloadError !== null && <ErrorNotice error={reloadError}/>}
      {reloaded && <div className="current-values" role="status"><strong>Сейчас в меню</strong><span>{serviceOptions.filter(service => basis.visible_services.includes(service.id)).map(service => service.label).join(', ') || 'Только разделы управления'}</span><button className="text-button" type="button" onClick={() => { setSelected(basis.visible_services); setReloaded(false); }}>Использовать сохранённый выбор</button></div>}
      {canManage ? <div className="settings-form-footer"><span className="subtle">Изменения видны всей команде</span><button className="button primary" type="submit" disabled={busy || !dirty || stale || mutation.isError}>{mutation.isPending ? 'Сохраняем…' : 'Сохранить сервисы'}<Icon name="check"/></button></div>
        : <p className="field-hint">Менять набор сервисов могут владелец и управляющие.</p>}
      {saved && <SuccessNotice>Сервисы в меню обновлены.</SuccessNotice>}
    </form>
  </section>;
}
