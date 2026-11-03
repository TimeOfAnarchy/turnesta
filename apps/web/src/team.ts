import { ApiError, request } from './api.ts';
import type { Role, Page } from './api.ts';
export type TeamRole = Exclude<Role, 'owner'>;
export type TeamMember = {
  user_id: string;
  display_name: string;
  email: string | null;
  role: Role;
  active: boolean;
  version: number;
  delivery_state: string | null;
  delivery_attempts: number | null;
  delivery_expires_at: string | null;
  resend_after: string | null;
};
export type TeamCommand =
  | { type: 'invite'; display_name: string; email: string; role: TeamRole }
  | {
      type: 'set_member';
      user_id: string;
      role: TeamRole;
      active: boolean;
      expected_version: number;
    }
  | { type: 'send_login'; user_id: string; expected_version: number };
export type TeamReceipt = {
  user_id: string;
  role: Role;
  active: boolean;
  version: number;
  delivery: { state: 'queued' | 'deferred'; retry_after: string } | null;
};
type Scope = { tenant: string; user: string };
type Pending = { key: string; command: TeamCommand };
type Storage = Pick<globalThis.Storage, 'getItem' | 'setItem' | 'removeItem'>;
const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i;
const roles = ['manager', 'operator', 'employee'];
export function validTeamCommand(value: unknown): value is TeamCommand {
  if (!value || typeof value !== 'object') return false;
  const v = value as Record<string, unknown>,
    keys = Object.keys(v).sort().join(',');
  if (v.type === 'invite')
    return (
      keys === 'display_name,email,role,type' &&
      typeof v.display_name === 'string' &&
      typeof v.email === 'string' &&
      v.email.length <= 254 &&
      v.display_name.length <= 240 &&
      roles.includes(v.role as string)
    );
  return (
    ((v.type === 'set_member' &&
      keys === 'active,expected_version,role,type,user_id' &&
      typeof v.active === 'boolean' &&
      roles.includes(v.role as string)) ||
      (v.type === 'send_login' && keys === 'expected_version,type,user_id')) &&
    typeof v.user_id === 'string' &&
    uuid.test(v.user_id) &&
    Number.isInteger(v.expected_version) &&
    (v.expected_version as number) > 0
  );
}
export function validTeamReceipt(value: unknown): value is TeamReceipt {
  if (!value || typeof value !== 'object') return false;
  const r = value as TeamReceipt;
  return (
    uuid.test(r.user_id) &&
    ['owner', ...roles].includes(r.role) &&
    typeof r.active === 'boolean' &&
    Number.isInteger(r.version) &&
    r.version > 0 &&
    (r.delivery === null ||
      Boolean(
        r.delivery &&
          ['queued', 'deferred'].includes(r.delivery.state) &&
          typeof r.delivery.retry_after === 'string' &&
          Number.isFinite(Date.parse(r.delivery.retry_after)),
      ))
  );
}
const url = (tenant: string) =>
  `/api/organizations/${encodeURIComponent(tenant)}/team`;
export const teamApi = {
  members: (tenant: string, after: string | null, signal: AbortSignal) =>
    request<Page<TeamMember>>(
      `${url(tenant)}?${new URLSearchParams({ limit: '50', ...(after ? { after } : {}) })}`,
      { signal },
    ),
  async send(scope: Scope, pending: Pending): Promise<TeamReceipt> {
    const result = await request<unknown>(`${url(scope.tenant)}/commands`, {
      method: 'POST',
      headers: { 'Idempotency-Key': pending.key },
      body: JSON.stringify(pending.command),
      signal: AbortSignal.timeout(10_000),
    });
    if (!validTeamReceipt(result)) throw new Error('Invalid team receipt');
    return result;
  },
};
export class TeamCommands {
  readonly scope: Scope;
  private storage: Storage | null;
  private send: typeof teamApi.send;
  readonly storageKey: string;
  pending: Pending | null = null;
  storageError: unknown = null;
  busy = false;
  constructor(scope: Scope, storage: Storage | null, send = teamApi.send) {
    this.scope = scope;
    this.storage = storage;
    this.send = send;
    this.storageKey = `turnesta:team-command:v1:${encodeURIComponent(scope.tenant)}:${encodeURIComponent(scope.user)}`;
    try {
      if (!storage) throw new Error('Storage unavailable');
      const raw = storage.getItem(this.storageKey);
      if (raw !== null) {
        const value: unknown = JSON.parse(raw);
        if (
          !value ||
          typeof value !== 'object' ||
          !('key' in value) ||
          typeof value.key !== 'string' ||
          !uuid.test(value.key) ||
          !('command' in value) ||
          !validTeamCommand(value.command)
        )
          throw new Error('Invalid saved team command');
        this.pending = { key: value.key, command: value.command };
      }
    } catch (error) {
      this.storageError = error;
    }
  }
  private clear() {
    try {
      this.storage!.removeItem(this.storageKey);
      this.pending = null;
    } catch (error) {
      this.storageError = error;
      throw error;
    }
  }
  async run(command?: TeamCommand): Promise<TeamReceipt> {
    if (this.storageError) throw this.storageError;
    if (this.busy || (command && this.pending) || (!command && !this.pending))
      throw new Error('Resolve the previous command');
    this.busy = true;
    try {
      if (command) {
        if (!validTeamCommand(command)) throw new Error('Invalid team command');
        const pending = {
          key: crypto.randomUUID(),
          command: structuredClone(command),
        };
        try {
          this.storage!.setItem(this.storageKey, JSON.stringify(pending));
        } catch (error) {
          this.storageError = error;
          throw error;
        }
        this.pending = pending;
      }
      let result: TeamReceipt;
      try {
        result = await this.send(this.scope, this.pending!);
        if (!validTeamReceipt(result)) throw new Error('Invalid team receipt');
      } catch (error) {
        if (
          error instanceof ApiError &&
          [
            'invalid_request',
            'invalid_name',
            'invalid_team_command',
            'team_member_unavailable',
            'team_rate_limited',
            'team_member_exists',
            'team_owner_protected',
            'team_version_conflict',
          ].includes(error.code)
        )
          this.clear();
        throw error;
      }
      this.clear();
      return result;
    } finally {
      this.busy = false;
    }
  }
}
export const teamRoleDescriptions: Record<Role, string> = {
  owner: 'Управление командой и все рабочие функции организации.',
  manager: 'Настройки точек, расписание, клиенты, очереди и чаевые.',
  operator:
    'Обслуживание очередей и запись клиентов, без управления настройками.',
  employee:
    'Просмотр расписаний и очередей, без контактов клиентов и управления.',
};
export function teamError(error: unknown): string | undefined {
  if (!(error instanceof ApiError)) return undefined;
  return (
    {
      invalid_name: 'Введите имя от 1 до 120 символов без управляющих символов.',
        account_email_unverified: 'Аккаунт создан в другой организации без подтверждения почты. Его владелец должен сначала подтвердить свой email через письмо для входа. После этого повторите приглашение.',
      team_member_exists:
        'Этот участник уже есть в команде. Обновите список и измените его доступ отдельно.',
      team_rate_limited:
        'Письмо уже запрошено недавно или достигнут лимит 20 запросов за час. Повторите позже.',
      team_version_conflict:
        'Доступ участника уже изменился. Обновите список и откройте его настройки заново.',
      team_member_unavailable:
        'Участник недоступен, его доступ отключён или к профилю ещё не привязан email.',
      team_owner_protected: 'Права владельца в этом разделе изменить нельзя.',
      invalid_team_command: 'Проверьте имя, email и выбранную роль.',
    } as Record<string, string>
  )[error.code];
}
