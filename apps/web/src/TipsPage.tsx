import { useEffect, useState } from 'react';
import type { FormEvent } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import type { InfiniteData } from '@tanstack/react-query';
import { Link } from 'react-router-dom';
import { api, ApiError, validateName } from './api';
import type { Page, TipAttempt, TipAttemptStatus, TipRecipientDraft } from './api';
import { useWorkspace } from './App';
import { ErrorNotice, Icon, Loading, Modal, SuccessNotice } from './components';
import { keys, useLocations, useTipAttempts, useTipRecipients } from './queries';
import { formatTipAmount, formatTipDate } from './tipsFormat';

const statusLabels: Record<TipAttemptStatus, string> = {
  awaiting_payment: 'Ожидает оплаты', succeeded: 'Успешно', failed: 'Не прошла', expired: 'Истекла',
};

function isDisabled(error: unknown): boolean {
  return error instanceof ApiError && error.code === 'tips_disabled';
}

export function TipsPage() {
  const { organization, canManage } = useWorkspace();
  useEffect(() => { document.title = `Чаевые · ${organization.name} · Turnesta`; }, [organization.name]);
  if (!canManage) return <>
    <div className="page-heading"><div><p className="eyebrow">ОРГАНИЗАЦИЯ / ЧАЕВЫЕ</p><h1>Благодарность за сервис<span className="heading-dot">.</span></h1><p className="page-intro">Чаевые для вашей команды.</p></div></div>
    <MockBanner/>
    <div className="panel empty-state"><span className="large-icon"><Icon name="heart"/></span><h2>Раздел для владельца и управляющих</h2><p>Управлять получателями чаевых и просматривать попытки оплаты могут владелец и управляющие организации.</p><Link className="button secondary" to={`/organizations/${organization.id}/locations`}>К точкам организации <Icon name="arrow"/></Link></div>
  </>;
  return <TipsManager key={organization.id} tenant={organization.id}/>;
}

function MockBanner() {
  return <div className="mock-banner" role="note"><span className="mock-banner-icon"><Icon name="heart"/></span><div><strong>Тестовый режим — деньги не списываются</strong><p>Получатели, страницы чаевых и попытки оплаты работают для проверки сценария. Реальные переводы и выплаты не выполняются.</p></div><span className="mock-badge">ТЕСТ</span></div>;
}

function TipsManager({ tenant }: { tenant: string }) {
  const recipients = useTipRecipients(tenant);
  const attempts = useTipAttempts(tenant);
  const locations = useLocations(tenant);
  const [adding, setAdding] = useState(false);
  const [success, setSuccess] = useState('');
  const recipientItems = recipients.data?.pages.flatMap((page) => page.items) ?? [];
  const attemptItems = attempts.data?.pages.flatMap((page) => page.items) ?? [];
  const locationNames = new Map(locations.data?.pages.flatMap((page) => page.items).map((location) => [location.id, location.name]));
  const disabled = isDisabled(recipients.error) || isDisabled(attempts.error);
  const denied = [recipients.error, attempts.error].find((error) => error instanceof ApiError && error.status === 403);

  function refresh() { void recipients.refetch(); void attempts.refetch(); }

  return <>
    <div className="page-heading"><div><p className="eyebrow">ОРГАНИЗАЦИЯ / ЧАЕВЫЕ</p><h1>Благодарность за сервис<span className="heading-dot">.</span></h1><p className="page-intro">Создайте страницу получателя и проверьте путь гостя до оплаты.</p></div>{!disabled && !denied && <button type="button" className="button primary" onClick={() => { setSuccess(''); setAdding(true); }}><Icon name="plus"/>Добавить получателя</button>}</div>
    <MockBanner/>
    {disabled ? <div className="panel empty-state"><span className="large-icon"><Icon name="heart"/></span><h2>Тестовые чаевые выключены</h2><p>Администратор сервиса может включить этот режим. Получать реальные переводы пока нельзя.</p><button type="button" className="button secondary" onClick={refresh} disabled={recipients.isFetching || attempts.isFetching}><Icon name="refresh"/>Проверить доступность</button></div> : denied ? <ErrorNotice error={denied} retry={refresh}/> : <>
      {success && <SuccessNotice>{success}</SuccessNotice>}
      <section className="tips-section" aria-labelledby="tip-recipients-title">
        <div className="section-line"><div className="section-title"><h2 id="tip-recipients-title">Получатели</h2>{recipientItems.length > 0 && <span className="count-badge">{recipientItems.length}{recipients.hasNextPage ? '+' : ''}</span>}</div><button type="button" className="text-button tips-refresh" onClick={() => { void recipients.refetch(); }} disabled={recipients.isFetching}><Icon name="refresh"/>{recipients.isFetching ? 'Обновляем…' : 'Обновить'}</button></div>
        {recipients.isPending ? <div className="panel"><Loading label="Загружаем получателей…"/></div> : recipients.isError && !recipientItems.length ? <ErrorNotice error={recipients.error} retry={() => { void recipients.refetch(); }}/> : recipientItems.length === 0 ? <div className="panel tips-empty"><span className="large-icon"><Icon name="heart"/></span><h3>У хорошего сервиса есть имя</h3><p>Добавьте получателя на существующей точке. У него появится отдельная тестовая страница чаевых.</p><button type="button" className="button secondary" onClick={() => { setSuccess(''); setAdding(true); }}><Icon name="plus"/>Добавить первого получателя</button></div> : <div className="tip-recipient-grid">{recipientItems.map((recipient) => <article key={recipient.id} className="panel tip-recipient-card">
          <div className="tip-recipient-heading"><span className="tip-recipient-avatar" aria-hidden="true">{[...recipient.display_name][0]?.toUpperCase()}</span><span className="mock-badge">ТЕСТ</span></div>
          <h3>{recipient.display_name}</h3><p className="tip-recipient-location"><Icon name="locations"/>{locationNames.get(recipient.location_id) ?? 'Точка организации'}</p>
          <a className="tip-public-link" href={`/tips/${encodeURIComponent(recipient.public_slug)}`} target="_blank" rel="noreferrer" aria-label={`Открыть тестовую страницу чаевых: ${recipient.display_name} (в новой вкладке)`}><span>Открыть тестовую страницу</span><Icon name="external"/></a>
          <p className="tip-link-hint">Тестовая ссылка — без списания денег</p>
        </article>)}</div>}
        {recipients.isError && recipientItems.length > 0 && <ErrorNotice error={recipients.error} retry={() => { void recipients.refetch(); }}/>}
        {recipients.hasNextPage && <div className="pagination"><button type="button" className="button secondary" onClick={() => { void recipients.fetchNextPage(); }} disabled={recipients.isFetching}>{recipients.isFetchingNextPage ? 'Загружаем…' : 'Показать ещё получателей'}</button></div>}
      </section>
      <section className="tips-section" aria-labelledby="tip-attempts-title">
        <div className="section-line"><div className="section-title"><h2 id="tip-attempts-title">Тестовые попытки оплаты</h2></div><button type="button" className="text-button tips-refresh" onClick={() => { void attempts.refetch(); }} disabled={attempts.isFetching}><Icon name="refresh"/>{attempts.isFetching ? 'Обновляем…' : 'Обновить'}</button></div>
        <p className="tips-section-description">Это история проверки сценария, а не поступившие деньги. Время указано для вашего устройства.</p>
        {attempts.isPending ? <div className="panel"><Loading label="Загружаем попытки оплаты…"/></div> : attempts.isError && !attemptItems.length ? <ErrorNotice error={attempts.error} retry={() => { void attempts.refetch(); }}/> : attemptItems.length === 0 ? <div className="panel tips-empty tips-attempts-empty"><span className="large-icon"><Icon name="check"/></span><h3>Попыток оплаты пока нет</h3><p>Откройте тестовую страницу получателя и пройдите сценарий гостя. Здесь появятся сумма и состояние попытки.</p></div> : <div className="panel tip-attempt-list"><div className="tip-attempt-columns" aria-hidden="true"><span>ПОЛУЧАТЕЛЬ / СОЗДАНО</span><span>СУММА · RUB</span><span>СОСТОЯНИЕ</span><span/></div>{attemptItems.map((attempt) => <AttemptRow key={attempt.id} attempt={attempt} tenant={tenant}/>)}</div>}
        {attempts.isError && attemptItems.length > 0 && <ErrorNotice error={attempts.error} retry={() => { void attempts.refetch(); }}/>}
        {attempts.hasNextPage && <div className="pagination"><button type="button" className="button secondary" onClick={() => { void attempts.fetchNextPage(); }} disabled={attempts.isFetching}>{attempts.isFetchingNextPage ? 'Загружаем…' : 'Показать ещё попытки'}</button></div>}
        <p className="tips-reconcile-hint">«Сверить» проверяет текущее состояние попытки. Выбрать результат оплаты в кабинете нельзя.</p>
      </section>
    </>}
    {adding && !disabled && !denied && <RecipientEditor tenant={tenant} close={() => setAdding(false)} saved={(name) => { setAdding(false); setSuccess(`Тестовый получатель «${name}» добавлен.`); }}/>}
  </>;
}

function AttemptRow({ attempt, tenant }: { attempt: TipAttempt; tenant: string }) {
  const client = useQueryClient();
  const reconciliation = useMutation({
    mutationFn: () => api.reconcileTipAttempt(tenant, attempt.id),
    retry: false,
    onSuccess: (actual) => {
      client.setQueryData<InfiniteData<Page<TipAttempt>>>(keys.tipAttempts(tenant), (previous) => previous ? {
        ...previous, pages: previous.pages.map((page) => ({ ...page, items: page.items.map((item) => item.id === actual.id ? actual : item) })),
      } : previous);
    },
  });
  return <article className="tip-attempt-row">
    <div className="tip-attempt-recipient"><h3>{attempt.recipient_name}</h3><time dateTime={attempt.created_at}>{formatTipDate(attempt.created_at)}</time></div>
    <div className="tip-attempt-amount"><strong>{formatTipAmount(attempt.amount_minor, attempt.currency)}</strong><span>Тестовая сумма</span></div>
    <span className={`tip-status status-${attempt.status}`}>{statusLabels[attempt.status] ?? 'Неизвестно'}</span>
    <button type="button" className="button secondary tip-reconcile-button" onClick={() => reconciliation.mutate()} disabled={reconciliation.isPending} aria-label={`Сверить попытку: ${attempt.recipient_name}, ${formatTipAmount(attempt.amount_minor, attempt.currency)}, ${formatTipDate(attempt.created_at)}`}><Icon name="refresh"/>{reconciliation.isPending ? 'Проверяем…' : 'Сверить'}</button>
    {reconciliation.isError && <div className="tip-attempt-notice"><ErrorNotice error={reconciliation.error}/></div>}
    {reconciliation.isSuccess && <p className="tip-reconcile-success" role="status"><Icon name="check"/>Состояние проверено. Реального списания не было.</p>}
  </article>;
}

function RecipientEditor({ tenant, close, saved }: { tenant: string; close: () => void; saved: (name: string) => void }) {
  const client = useQueryClient();
  const locations = useLocations(tenant);
  const choices = locations.data?.pages.flatMap((page) => page.items) ?? [];
  const [draft, setDraft] = useState<TipRecipientDraft>({ display_name: '', location_id: '' });
  const [errors, setErrors] = useState<{ display_name?: string; location_id?: string }>({});
  const mutation = useMutation({
    mutationFn: (value: TipRecipientDraft) => api.createTipRecipient(tenant, value),
    retry: false,
    onSuccess: (recipient) => { void client.invalidateQueries({ queryKey: keys.tipRecipients(tenant) }); saved(recipient.display_name); },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    const next = { display_name: validateName(draft.display_name), location_id: choices.some((location) => location.id === draft.location_id) ? undefined : 'Выберите точку организации.' };
    setErrors(next);
    if (!next.display_name && !next.location_id) mutation.mutate({ display_name: draft.display_name.trim(), location_id: draft.location_id });
  }

  return <Modal title="Новый получатель" description="Получатель получит тестовую страницу чаевых. Реальные переводы пока недоступны." close={close} busy={mutation.isPending}>
    <form className="editor-form" onSubmit={submit} noValidate>
      <div className="form-field"><label htmlFor="recipient-display-name">Имя на странице чаевых</label><input id="recipient-display-name" name="display_name" value={draft.display_name} autoComplete="off" autoFocus disabled={mutation.isPending} onChange={(event) => { setDraft({ ...draft, display_name: event.target.value }); setErrors({ ...errors, display_name: undefined }); }} aria-invalid={Boolean(errors.display_name)} aria-describedby={errors.display_name ? 'recipient-name-error' : 'recipient-name-hint'}/>{errors.display_name ? <p className="field-error" id="recipient-name-error">{errors.display_name}</p> : <p className="field-hint" id="recipient-name-hint">Гости увидят это имя перед тестовой оплатой. От 1 до 120 символов.</p>}</div>
      <div className="form-field"><label htmlFor="recipient-location">Точка</label><select id="recipient-location" name="location_id" value={draft.location_id} disabled={mutation.isPending || locations.isPending || choices.length === 0} onChange={(event) => { setDraft({ ...draft, location_id: event.target.value }); setErrors({ ...errors, location_id: undefined }); }} aria-invalid={Boolean(errors.location_id)} aria-describedby={errors.location_id ? 'recipient-location-error' : 'recipient-location-hint'}><option value="">{locations.isPending ? 'Загружаем точки…' : 'Выберите точку'}</option>{choices.map((location) => <option key={location.id} value={location.id}>{location.name}</option>)}</select>{errors.location_id ? <p className="field-error" id="recipient-location-error">{errors.location_id}</p> : <p className="field-hint" id="recipient-location-hint">Получатель относится к одной из точек вашей организации.</p>}
        {locations.isError && <ErrorNotice error={locations.error} retry={() => { void locations.refetch(); }}/>}
        {!locations.isPending && !locations.isError && choices.length === 0 && <p className="field-hint">Сначала <Link to={`/organizations/${tenant}/locations`}>добавьте точку организации</Link>, затем создайте получателя.</p>}
        {locations.hasNextPage && <button type="button" className="text-button tip-more-locations" onClick={() => { void locations.fetchNextPage(); }} disabled={locations.isFetching || mutation.isPending}>{locations.isFetchingNextPage ? 'Загружаем…' : 'Показать ещё точки'}</button>}
      </div>
      {mutation.isError && <ErrorNotice error={mutation.error}/>}
      <div className="dialog-actions"><button type="button" className="button secondary" onClick={close} disabled={mutation.isPending}>Отмена</button><button type="submit" className="button primary" disabled={mutation.isPending || choices.length === 0}>{mutation.isPending ? 'Добавляем…' : 'Добавить получателя'}{!mutation.isPending && <Icon name="plus"/>}</button></div>
    </form>
  </Modal>;
}
