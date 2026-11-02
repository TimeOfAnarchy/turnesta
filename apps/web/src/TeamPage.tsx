import { useEffect, useState } from 'react';
import type { FormEvent } from 'react';
import { useInfiniteQuery } from '@tanstack/react-query';
import { useWorkspace } from './App';
import { ErrorNotice, Icon, Loading, Modal, SuccessNotice } from './components';
import { roleLabels, errorMessage, validateName } from './api';
import { TeamCommands, teamApi, teamError, teamRoleDescriptions } from './team';
import type { TeamMember, TeamCommand, TeamRole } from './team';

export function TeamPage() {
  const workspace = useWorkspace();
  useEffect(() => {
    document.title = `Команда · ${workspace.organization.name} · Turnesta`;
  }, [workspace.organization.name]);
  if (workspace.role !== 'owner')
    return (
      <div className="panel empty-state">
        <Icon name="users" />
        <h1>Доступами управляет владелец</h1>
        <p>
          Обратитесь к владельцу организации, чтобы пригласить коллегу или
          изменить роль.
        </p>
      </div>
    );
  return (
    <Team
      key={`${workspace.organization.id}:${workspace.userId}`}
      tenant={workspace.organization.id}
      user={workspace.userId}
    />
  );
}
function Team({ tenant, user }: { tenant: string; user: string }) {
  const members = useInfiniteQuery({
    queryKey: ['organization', tenant, 'team'],
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) =>
      teamApi.members(tenant, pageParam, signal),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    refetchInterval: 10_000,
    retry: false,
  });
  const [journal] = useState(() => {
    let storage: Storage | null = null;
    try {
      storage = window.sessionStorage;
    } catch {
      /* The journal reports unavailable storage. */
    }
    return new TeamCommands({ tenant, user }, storage);
  });
  const [busy, setBusy] = useState(false),
    [error, setError] = useState<unknown>(null),
    [success, setSuccess] = useState('');
  const [editor, setEditor] = useState<TeamMember | 'invite' | null>(null);
  const items = members.data?.pages.flatMap((p) => p.items) ?? [];
  const locked = busy || Boolean(journal.storageError || journal.pending);
  async function run(command?: TeamCommand) {
    let saved = false;
    setBusy(true);
    setError(null);
    setSuccess('');
    try {
      const receipt = await journal.run(command);
      setSuccess(
        receipt.delivery
          ? receipt.delivery.state === 'queued'
            ? 'Доступ предоставлен. Письмо для входа поставлено в очередь; ссылка действует 10 минут.'
            : `Доступ предоставлен. Новое письмо можно запросить после ${new Date(receipt.delivery.retry_after).toLocaleString('ru-RU')}. Уже действующая ссылка входа сохранена.`
          : receipt.active
            ? 'Роль и доступ участника сохранены.'
            : 'Доступ к организации отключён. История работы сохранена.',
      );
      await members.refetch();
      saved = true;
    } catch (e) {
      setError(e);
      throw e;
    } finally {
      setBusy(false);
      if (saved) setEditor(null);
    }
  }
  return (
    <>
      <div className="page-heading">
        <div>
          <p className="eyebrow">ОРГАНИЗАЦИЯ / КОМАНДА</p>
          <h1>
            Люди вашего бизнеса<span className="heading-dot">.</span>
          </h1>
          <p className="page-intro">
            Приглашайте коллег и выбирайте, с чем они смогут работать.
          </p>
        </div>
        <span className="heading-decoration" aria-hidden="true">
          <Icon name="users" />
        </span>
      </div>
      <div className="team-overview panel">
        <div>
          <span className="small-eyebrow">ОДНА КОМАНДА</span>
          <h2>Доступ ко всем точкам организации</h2>
          <p>
            Роль действует в очередях, записи и других рабочих разделах.
            Управление командой остаётся у владельца.
          </p>
        </div>
        <button
          type="button"
          className="button primary"
          disabled={locked}
          onClick={() => {
            setError(null);
            setEditor('invite');
          }}
        >
          <Icon name="plus" />
          Пригласить коллегу
        </button>
      </div>
      {journal.storageError !== null && (
        <div className="error-notice" role="alert">
          <p>
            Не удалось открыть журнал действий в браузере. Обновите страницу или
            разрешите хранилище для Turnesta; изменения пока недоступны.
          </p>
        </div>
      )}
      {journal.pending && (
        <div className="conflict-notice" role="alert">
          <strong>Результат действия ещё не подтверждён</strong>
          <p>
            Восстановите прежний запрос, чтобы проверить результат и избежать
            повторных приглашений.
          </p>
          <button
            type="button"
            className="button secondary"
            disabled={busy}
            onClick={() => void run().catch(() => {})}
          >
            Восстановить действие
          </button>
        </div>
      )}
      {success && <SuccessNotice>{success}</SuccessNotice>}
      {error !== null && (
        <div className="error-notice" role="alert">
          <p>{teamError(error) ?? errorMessage(error)}</p>
        </div>
      )}
      <div className="section-line">
        <h2>Участники</h2>
        <button
          type="button"
          className="text-button team-refresh"
          onClick={() => void members.refetch()}
          disabled={members.isFetching}
        >
          Обновить список <Icon name="refresh" />
        </button>
      </div>
      {members.isPending ? (
        <Loading label="Загружаем команду…" />
      ) : members.isError && !items.length ? (
        <ErrorNotice
          error={members.error}
          retry={() => void members.refetch()}
        />
      ) : (
        <div className="team-members">
          {items.map((member) => (
            <article
              className={`team-member panel${member.active ? '' : ' team-member-disabled'}`}
              key={member.user_id}
            >
              <span className="team-avatar" aria-hidden="true">
                {[...member.display_name][0]?.toLocaleUpperCase('ru-RU')}
              </span>
              <div className="team-member-copy">
                <h3>
                  {member.display_name}
                  {member.user_id === user && (
                    <span className="subtle"> · вы</span>
                  )}
                </h3>
                <p>{member.email ?? 'Email ещё не привязан'}</p>
                <p className="field-hint">
                  {teamRoleDescriptions[member.role]}
                </p>
                {member.delivery_state && (
                  <p className="team-delivery">{deliveryLabel(member)}</p>
                )}
              </div>
              <div className="team-member-actions">
                <span
                  className={`role-badge${member.active ? '' : ' team-disabled-badge'}`}
                >
                  {member.active ? roleLabels[member.role] : 'Доступ отключён'}
                </span>
                {member.role !== 'owner' && (
                  <button
                    className="button secondary"
                    type="button"
                    disabled={locked}
                    onClick={() => {
                      setError(null);
                      setEditor(member);
                    }}
                  >
                    Настроить доступ
                  </button>
                )}
                {member.role !== 'owner' && member.active && member.email && (
                  <button
                    className="text-button"
                    type="button"
                    disabled={
                      locked ||
                      Boolean(
                        member.resend_after &&
                          Date.parse(member.resend_after) > Date.now(),
                      )
                    }
                    onClick={() =>
                      void run({
                        type: 'send_login',
                        user_id: member.user_id,
                        expected_version: member.version,
                      }).catch(() => {})
                    }
                  >
                    Отправить письмо для входа
                  </button>
                )}
              </div>
            </article>
          ))}
        </div>
      )}
      {members.isError && items.length > 0 && (
        <ErrorNotice
          error={members.error}
          retry={() => void members.refetch()}
        />
      )}
      {members.hasNextPage && (
        <div className="pagination">
          <button
            className="button secondary"
            disabled={members.isFetchingNextPage}
            onClick={() => void members.fetchNextPage()}
          >
            Показать ещё участников
          </button>
        </div>
      )}
      <details className="team-role-guide panel">
        <summary>Что доступно каждой роли</summary>
        <div className="team-role-grid">
          {(['owner', 'manager', 'operator', 'employee'] as const).map(
            (role) => (
              <div key={role}>
                <strong>{roleLabels[role]}</strong>
                <p>{teamRoleDescriptions[role]}</p>
              </div>
            ),
          )}
        </div>
        <p className="field-hint">
          Профиль специалиста в расписании не даёт входа в кабинет. Для работы в
          Turnesta человека нужно добавить в команду.
        </p>
      </details>
      {editor && (
        <TeamEditor
          member={editor === 'invite' ? null : editor}
          busy={busy}
          pending={Boolean(journal.pending)}
          run={run}
          close={() => setEditor(null)}
        />
      )}
    </>
  );
}
function deliveryLabel(m: TeamMember) {
  if (m.delivery_state === 'deferred')
    return m.resend_after && Date.parse(m.resend_after) > Date.now()
      ? `Новое письмо можно запросить после ${new Date(m.resend_after).toLocaleString('ru-RU')}.`
      : 'Можно снова запросить письмо для входа.';
  const expired =
    m.delivery_expires_at && Date.parse(m.delivery_expires_at) <= Date.now();
  if (expired) return 'Срок ссылки для входа истёк.';
  return (
    (
      {
        pending: 'Письмо ожидает отправки',
        leased: 'Письмо отправляется',
        sent: 'Письмо передано почтовому серверу',
        failed: 'Письмо не удалось отправить',
        expired: 'Срок ссылки для входа истёк',
        canceled: 'Предыдущее письмо отменено',
      } as Record<string, string>
    )[m.delivery_state!] ?? 'Проверяем доставку письма'
  );
}
function TeamEditor({
  member,
  busy,
  pending,
  run,
  close,
}: {
  member: TeamMember | null;
  busy: boolean;
  pending: boolean;
  run: (command: TeamCommand) => Promise<void>;
  close: () => void;
}) {
  const [name, setName] = useState(''),
    [email, setEmail] = useState(''),
    [role, setRole] = useState<TeamRole>(
      (member?.role as TeamRole) ?? 'operator',
    ),
    [active, setActive] = useState(member?.active ?? true),
    [error, setError] = useState<unknown>(null);
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (busy || pending) return;
    setError(null);
    if (!member) {
      const issue = validateName(name);
      if (issue) {
        setError(issue);
        return;
      }
    }
    try {
      await run(
        member
          ? {
              type: 'set_member',
              user_id: member.user_id,
              role,
              active,
              expected_version: member.version,
            }
          : {
              type: 'invite',
              display_name: name.trim(),
              email: email.trim(),
              role,
            },
      );
    } catch (e) {
      setError(e);
    }
  }
  return (
    <Modal
      title={member ? 'Доступ участника' : 'Пригласить коллегу'}
      description={
        member
          ? `${member.display_name} · ${member.email ?? 'без email'}`
          : 'Доступ появится сразу. Для входа отправим одноразовую ссылку на email.'
      }
      busy={busy}
      close={close}
    >
      <form className="team-form" onSubmit={submit}>
        <fieldset disabled={busy || pending}>
          {!member && (
            <>
              <label htmlFor="team-name">Имя коллеги</label>
              <input
                id="team-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                maxLength={120}
                required
                autoComplete="name"
              />
              <p className="field-hint">
                Имя для нового профиля. Если коллега уже пользуется Turnesta,
                сохраним его имя.
              </p>
              <label htmlFor="team-email">Email для входа</label>
              <input
                id="team-email"
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                maxLength={254}
                required
                autoComplete="email"
              />
            </>
          )}
          <label htmlFor="team-role">Роль в организации</label>
          <select
            id="team-role"
            value={role}
            onChange={(e) => setRole(e.target.value as TeamRole)}
          >
            {(['manager', 'operator', 'employee'] as const).map((r) => (
              <option key={r} value={r}>
                {roleLabels[r]}
              </option>
            ))}
          </select>
          <p className="field-hint">{teamRoleDescriptions[role]}</p>
          {member && (
            <label className="checkbox-field">
              <input
                type="checkbox"
                checked={active}
                onChange={(e) => setActive(e.target.checked)}
              />
              Доступ к организации включён
            </label>
          )}
          {member && !active && (
            <p className="team-access-note">
              После сохранения участник потеряет доступ ко всем точкам этой
              организации. Прошлые действия и клиентские записи сохранятся.
            </p>
          )}
          {!member && (
            <p className="team-access-note">
              Роль действует для всех точек организации. Перед приглашением
              проверьте адрес и нужный уровень доступа.
            </p>
          )}
        </fieldset>
        {error !== null && (
          <p className="field-error" role="alert">
            {typeof error === 'string'
              ? error
              : (teamError(error) ?? errorMessage(error))}
          </p>
        )}
        {pending && (
          <p className="field-hint" role="status">
            Результат неизвестен. Закройте окно и восстановите действие из
            уведомления над списком команды.
          </p>
        )}
        <div className="form-actions">
          <button
            className="button primary"
            disabled={busy || pending}
            type="submit"
          >
            {busy
              ? 'Сохраняем…'
              : member
                ? 'Сохранить доступ'
                : 'Пригласить и отправить письмо'}
          </button>
        </div>
      </form>
    </Modal>
  );
}
