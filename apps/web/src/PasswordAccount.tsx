import { useState } from 'react';
import type { FormEvent } from 'react';
import { ApiError, request, roleLabels, validateName } from './api';
import { Icon, Modal } from './components';
import type { TeamRole } from './team';

function temporaryPassword() {
  return Array.from(crypto.getRandomValues(new Uint8Array(18)), b => b.toString(16).padStart(2, '0')).join('');
}

export function PasswordAccountButton({ tenant, disabled, onCreated }: { tenant: string; disabled: boolean; onCreated: () => void }) {
  const [open, setOpen] = useState(false);
  return <>
    <button className="button primary" type="button" disabled={disabled} onClick={() => setOpen(true)}><Icon name="plus"/>Добавить сотрудника</button>
    {open && <PasswordAccount tenant={tenant} close={() => setOpen(false)} onCreated={onCreated}/>}
  </>;
}

function PasswordAccount({ tenant, close, onCreated }: { tenant: string; close: () => void; onCreated: () => void }) {
  const [name, setName] = useState('');
  const [email, setEmail] = useState('');
  const [role, setRole] = useState<TeamRole>('operator');
  // Kept only in this mounted form. Never place credentials in the command journal.
  const [password, setPassword] = useState(temporaryPassword);
  const [visible, setVisible] = useState(false);
  const [busy, setBusy] = useState(false);
  const [created, setCreated] = useState(false);
  const [error, setError] = useState('');
  const [copied, setCopied] = useState(false);
  async function submit(event: FormEvent) {
    event.preventDefault();
    if (busy || created) return;
    const issue = validateName(name);
    if (issue) { setError(issue); return; }
    setBusy(true); setError('');
    try {
      await request(`/api/organizations/${encodeURIComponent(tenant)}/team/password-accounts`, {
        method: 'POST', body: JSON.stringify({ display_name: name.trim(), email: email.trim(), role, temporary_password: password }),
      });
      setCreated(true); onCreated();
    } catch (error) {
      setError(error instanceof ApiError && error.code === 'email_in_use'
        ? 'Для этого email уже есть аккаунт. Пароль сохранён прежним. Добавьте существующий аккаунт через приглашение по email; если предыдущий запрос был прерван, проверьте список команды.'
        : error instanceof ApiError && error.status === 429
          ? 'Лимит создания аккаунтов достигнут. Попробуйте позже.'
          : error instanceof ApiError && error.code === 'invalid_password'
            ? 'Используйте временный пароль от 15 до 128 символов.'
            : 'Результат не подтверждён. Обновите список команды перед повтором: аккаунт уже мог быть создан. Этот пароль не сохраняется после закрытия формы.');
    } finally { setBusy(false); }
  }
  async function copy() {
    try {
      await navigator.clipboard.writeText(`Вход: ${location.origin}/login\nEmail: ${email.trim()}\nВременный пароль: ${password}\nПри первом входе задайте свой пароль.`);
      setCopied(true);
    } catch { setError('Не удалось скопировать. Покажите пароль и передайте данные вручную.'); }
  }
  return <Modal title={created ? 'Доступ сотрудника создан' : 'Новый сотрудник'} description={created
    ? 'Передайте данные сотруднику. После первого входа он задаст свой пароль. Временный пароль виден только в этой форме.'
    : 'Создайте новый аккаунт с email и временным паролем. Доступ будет действовать в этой организации.'}
    close={close} busy={busy} icon="users">
    <form onSubmit={event => void submit(event)} className="team-form" autoComplete="off">
      {error && <p className="error-notice" role="alert">{error}</p>}
      <label htmlFor="password-member-name">Имя сотрудника</label>
      <input id="password-member-name" required maxLength={120} value={name} disabled={busy} readOnly={created} onChange={e => setName(e.target.value)}/>
      <label htmlFor="password-member-email">Email</label>
      <input id="password-member-email" type="email" required maxLength={254} autoCapitalize="off" spellCheck={false} value={email} disabled={busy} readOnly={created} onChange={e => setEmail(e.target.value)}/>
      <label htmlFor="password-member-role">Роль</label>
      <select id="password-member-role" value={role} disabled={busy || created} onChange={e => setRole(e.target.value as TeamRole)}>
        {(['manager', 'operator', 'employee'] as const).map(role => <option key={role} value={role}>{roleLabels[role]}</option>)}
      </select>
      <label htmlFor="password-member-secret">Временный пароль</label>
      <input id="password-member-secret" type={visible ? 'text' : 'password'} autoComplete="new-password" required minLength={15} maxLength={128} value={password} disabled={busy} readOnly={created} onChange={e => setPassword(e.target.value)}/>
      <div className="form-actions">
        <button className="button secondary" type="button" disabled={busy} onClick={() => setVisible(!visible)}>{visible ? 'Скрыть пароль' : 'Показать пароль'}</button>
        {!created && <button className="button secondary" type="button" disabled={busy} onClick={() => setPassword(temporaryPassword())}>Сгенерировать другой</button>}
      </div>
      <p className="field-hint">Сохраните данные до закрытия формы. Для существующего аккаунта пароль здесь не меняется.</p>
      {created && <p className="success-notice" role="status">{copied ? 'Данные скопированы.' : 'Аккаунт создан. Передайте данные сотруднику.'}</p>}
      <div className="form-actions">
        <button className="button secondary" type="button" disabled={busy} onClick={close}>{created ? 'Готово' : 'Закрыть'}</button>
        {created ? <button className="button primary" type="button" onClick={() => void copy()}>Копировать данные входа</button>
          : <button className="button primary" type="submit" disabled={busy}>{busy ? 'Создаём…' : 'Создать доступ'}</button>}
      </div>
    </form>
  </Modal>;
}
