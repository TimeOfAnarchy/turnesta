import { useEffect, useState } from 'react';
import type { FormEvent } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { api, ApiError, validateName } from './api';
import type { Organization } from './api';
import { useWorkspace } from './App';
import { ErrorNotice, Icon, SuccessNotice } from './components';
import { keys } from './queries';
import { NavigationSettings } from './NavigationSettings';

export function SettingsPage() {
  const { organization, canManage } = useWorkspace();
  useEffect(() => { document.title = `Настройки · ${organization.name} · Turnesta`; }, [organization.name]);
  return <>
    <div className="page-heading"><div><p className="eyebrow">ОРГАНИЗАЦИЯ / НАСТРОЙКИ</p><h1>Всё на своих местах<span className="heading-dot">.</span></h1><p className="page-intro">Основная информация и сервисы вашей организации.</p></div><span className="heading-decoration" aria-hidden="true"><Icon name="settings"/></span></div>
    <div className="settings-layout"><div className="settings-panels"><section className="panel settings-panel" aria-labelledby="organization-details-heading"><div className="panel-heading"><span className="square-icon"><Icon name="grid"/></span><div><h2 id="organization-details-heading">Об организации</h2><p>Название вашего рабочего пространства.</p></div></div>
      {canManage ? <OrganizationForm key={organization.id} organization={organization}/> : <div className="readonly-details"><span className="field-label">Название организации</span><strong>{organization.name}</strong><p className="field-hint">Менять настройки могут владелец и управляющие.</p></div>}
    </section><NavigationSettings key={organization.id} organization={organization} canManage={canManage}/><BookingRetentionSettings key={organization.id} organization={organization} canManage={canManage}/></div><aside className="settings-aside"><span className="small-eyebrow">ЕДИНОЕ ПРОСТРАНСТВО</span><h3>Кабинет под ваш бизнес</h3><p>Название и набор разделов общие для команды. Для клиники можно оставить запись, для сервисной точки — очереди.</p><div className="aside-rule"/><p className="aside-footnote">Точки и их часовые пояса настраиваются отдельно в разделе «Точки».</p></aside></div>
  </>;
}

export function BookingRetentionSettings({ organization, canManage }: { organization: Organization; canManage: boolean }) {
  const client = useQueryClient();
  const [basis, setBasis] = useState(organization);
  const [months, setMonths] = useState(organization.booking_retention_months?.toString() ?? '');
  const [saved, setSaved] = useState(false);
  const dirty = months !== (basis.booking_retention_months?.toString() ?? '');
  const changedElsewhere = basis.version !== organization.version;
  useEffect(() => {
    if (!dirty && changedElsewhere) { setBasis(organization); setMonths(organization.booking_retention_months?.toString() ?? ''); }
  }, [basis.version, changedElsewhere, dirty, organization]);
  const mutation = useMutation({
    mutationFn: () => api.setBookingRetention(organization.id, months ? Number(months) : null, basis.version),
    onSuccess: value => { setBasis(value); setMonths(value.booking_retention_months?.toString() ?? ''); setSaved(true); client.setQueryData(keys.organization(organization.id), value); },
  });
  const valid = months === '' || (/^\d+$/.test(months) && Number(months) >= 1 && Number(months) <= 1200);
  return <section className="panel settings-panel" aria-labelledby="booking-retention-heading">
    <div className="panel-heading"><span className="square-icon"><Icon name="settings"/></span><div><h2 id="booking-retention-heading">Хранение данных клиентов</h2><p>Автоматическое удаление данных из старых записей.</p></div></div>
    <p className="field-hint retention-description">После указанного срока очищаются имя, контакты, примечание и ответы. Время и факт записи сохраняются. Записи без известного окончания автоматически не очищаются; отменённые отсчитываются от отмены.</p>
    {canManage ? <form className="organization-form" onSubmit={event => { event.preventDefault(); if (valid && dirty && !changedElsewhere) { setSaved(false); mutation.mutate(); } }}>
      <div className="form-field"><label htmlFor="booking-retention-months">Срок в месяцах</label><input id="booking-retention-months" type="number" min="1" max="1200" value={months} onChange={event => { setMonths(event.target.value); setSaved(false); }} disabled={mutation.isPending}/><p className="field-hint">Оставьте поле пустым, чтобы отключить автоматическое удаление.</p></div>
      {changedElsewhere && <div className="conflict-notice" role="alert"><strong>Настройки изменились</strong><p>Загрузите текущие значения, прежде чем сохранить свой вариант.</p><button type="button" className="text-button" onClick={() => { setBasis(organization); setMonths(organization.booking_retention_months?.toString() ?? ''); mutation.reset(); }}>Использовать текущие значения</button></div>}
      {mutation.isError && <ErrorNotice error={mutation.error}/>}
      <div className="settings-form-footer"><span className="subtle">Резервные копии не изменяются</span><button type="submit" className="button primary" disabled={!valid || !dirty || changedElsewhere || mutation.isPending}>Сохранить</button></div>
      {saved && <SuccessNotice>Срок хранения обновлён.</SuccessNotice>}
    </form> : <div className="readonly-details"><span className="field-label">Срок хранения</span><strong>{organization.booking_retention_months === null ? 'Автоматическое удаление выключено' : `${organization.booking_retention_months} мес.`}</strong></div>}
  </section>;
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
  useEffect(() => {
    if (!dirty && !mutation.isPending && organization.version !== basis.version) {
      setBasis(organization); setName(organization.name); setSaved(false);
    }
  }, [organization, basis.version, dirty, mutation.isPending]);

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
