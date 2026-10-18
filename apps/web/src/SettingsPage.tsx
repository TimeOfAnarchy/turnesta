import { useEffect, useState } from 'react';
import type { FormEvent } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { api, ApiError, validateName } from './api';
import type { Organization } from './api';
import { useWorkspace } from './App';
import { ErrorNotice, Icon, SuccessNotice } from './components';
import { keys } from './queries';

export function SettingsPage() {
  const { organization, canManage } = useWorkspace();
  useEffect(() => { document.title = `Настройки · ${organization.name} · Turnesta`; }, [organization.name]);
  return <>
    <div className="page-heading"><div><p className="eyebrow">ОРГАНИЗАЦИЯ / НАСТРОЙКИ</p><h1>Всё на своих местах<span className="heading-dot">.</span></h1><p className="page-intro">Основная информация о вашей организации.</p></div><span className="heading-decoration" aria-hidden="true"><Icon name="settings"/></span></div>
    <div className="settings-layout"><section className="panel settings-panel" aria-labelledby="organization-details-heading"><div className="panel-heading"><span className="square-icon"><Icon name="grid"/></span><div><h2 id="organization-details-heading">Об организации</h2><p>Название вашего рабочего пространства.</p></div></div>
      {canManage ? <OrganizationForm key={organization.id} organization={organization}/> : <div className="readonly-details"><span className="field-label">Название организации</span><strong>{organization.name}</strong><p className="field-hint">Менять настройки могут владелец и управляющие.</p></div>}
    </section><aside className="settings-aside"><span className="small-eyebrow">ЕДИНОЕ ПРОСТРАНСТВО</span><h3>Одно имя для всей команды</h3><p>Название отображается в кабинете и помогает быстро найти нужную организацию.</p><div className="aside-rule"/><p className="aside-footnote">Точки и их часовые пояса настраиваются отдельно в разделе «Точки».</p></aside></div>
  </>;
}

function OrganizationForm({ organization }: { organization: Organization }) {
  const client = useQueryClient();
  const [basis, setBasis] = useState(organization);
  const [name, setName] = useState(organization.name);
  const [validation, setValidation] = useState<string>();
  const [saved, setSaved] = useState(false);
  const [reloading, setReloading] = useState(false);
  const [reloaded, setReloaded] = useState(false);
  const [reloadError, setReloadError] = useState<unknown>(null);
  const mutation = useMutation({
    mutationFn: () => api.renameOrganization(organization.id, name.trim(), basis.version),
    onSuccess: (value) => {
      setBasis(value); setName(value.name); setSaved(true); setReloaded(false);
      client.setQueryData(keys.organization(organization.id), value);
      void client.invalidateQueries({ queryKey: keys.organizations });
    },
  });
  const conflict = mutation.error instanceof ApiError && mutation.error.code === 'version_conflict';
  const changedElsewhere = organization.version !== basis.version;
  const dirty = name.trim() !== basis.name;
  const busy = mutation.isPending || reloading;

  function submit(event: FormEvent) {
    event.preventDefault();
    const issue = validateName(name);
    setValidation(issue); setSaved(false);
    if (!issue) mutation.mutate();
  }
  async function reload() {
    setReloading(true); setReloadError(null);
    try {
      const actual = await client.fetchQuery({ queryKey: keys.organization(organization.id), queryFn: ({ signal }) => api.organization(organization.id, signal), staleTime: 0 });
      setBasis(actual); mutation.reset(); setReloaded(true); setSaved(false);
    } catch (error) { setReloadError(error); }
    finally { setReloading(false); }
  }
  return <form className="organization-form" onSubmit={submit} noValidate>
    <div className="form-field"><label htmlFor="organization-name">Название организации</label><input id="organization-name" name="name" value={name} onChange={(event) => { setName(event.target.value); setValidation(undefined); setSaved(false); }} disabled={busy} autoComplete="off" aria-invalid={Boolean(validation)} aria-describedby={validation ? 'organization-name-error' : 'organization-name-hint'}/>{validation ? <p className="field-error" id="organization-name-error">{validation}</p> : <p className="field-hint" id="organization-name-hint">От 1 до 120 символов. Название можно изменить позже.</p>}</div>
    {(conflict || changedElsewhere) && <div className="conflict-notice" role="alert"><strong>Настройки уже изменились</strong><p>Ваш вариант остался в поле. Загрузите текущую версию, чтобы сравнить значения перед сохранением.</p><button type="button" className="text-button" disabled={busy} onClick={() => { void reload(); }}>{reloading ? 'Загружаем…' : 'Загрузить текущую версию'}</button></div>}
    {reloaded && <div className="current-values" role="status"><strong>Сейчас сохранено</strong><span>{basis.name}</span><p>При сохранении ваш вариант заменит текущее название.</p><button type="button" className="text-button" onClick={() => { setName(basis.name); setValidation(undefined); setReloaded(false); }}>Использовать текущее название</button></div>}
    {mutation.isError && !conflict && <ErrorNotice error={mutation.error}/>}{reloadError !== null && <ErrorNotice error={reloadError}/>}
    <div className="settings-form-footer"><span className="subtle">Изменения видны всей команде</span><button type="submit" className="button primary" disabled={busy || conflict || changedElsewhere || !dirty}>{mutation.isPending ? 'Сохраняем…' : 'Сохранить'}{!mutation.isPending && <Icon name="check"/>}</button></div>
    {saved && <SuccessNotice>Название организации обновлено.</SuccessNotice>}
  </form>;
}
