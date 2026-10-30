import { api, ApiError } from './api.ts';
import type { QueueCommand, QueueCommandResult, Role, Ticket } from './api.ts';

export type CommandScope = { tenant: string; queue: string; user: string };
export type PendingCommand = { key: string; command: QueueCommand; created_at: string };
type JournalStorage = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;
type SendCommand = (scope: CommandScope, pending: PendingCommand) => Promise<QueueCommandResult>;

export const ticketStatusLabels = {
  waiting: 'Ожидает', called: 'Обслуживается', completed: 'Завершён', cancelled: 'Отменён',
};

export function canCloseTicket(ticket: Ticket, role: Role, user: string): boolean {
  return role !== 'employee' && (ticket.status === 'waiting' || ticket.status === 'called')
    && (ticket.called_by === null || ticket.called_by === user || role === 'owner' || role === 'manager');
}

export function ticketLabel(ticket: Pick<Ticket, 'number' | 'service_date'>): string {
  const [year, month, day] = ticket.service_date.split('-');
  return `№ ${String(ticket.number).padStart(3, '0')} · ${day}.${month}.${year}`;
}

export function commandLabel(command: QueueCommand): string {
  return { issue: 'Выдача талона', call_next: 'Вызов следующего', complete: 'Завершение обслуживания', cancel: 'Отмена талона' }[command.type];
}

export function commandError(error: unknown): string | undefined {
  if (!(error instanceof ApiError)) return undefined;
  if (error.code === 'active_ticket') return 'У вас уже есть вызванный посетитель. Завершите или отмените его обслуживание.';
  if (error.code === 'service_unavailable') return 'Услуга недоступна. Выберите другую или выдайте талон без уточнения услуги.';
  if (error.code === 'state_conflict') return 'Состояние талона уже изменилось. Список обновлён — проверьте его перед следующим действием.';
  if (error.code === 'idempotency_conflict') return 'Не удалось восстановить прежнее действие: запрос отличается от сохранённого. Обратитесь к администратору сервиса.';
  return undefined;
}

function validCommand(value: unknown): value is QueueCommand {
  if (!value || typeof value !== 'object' || !('type' in value)) return false;
  const keys = Object.keys(value);
  if (value.type === 'call_next') return keys.length === 1;
  if (value.type === 'issue') return keys.length === 1 || keys.length === 2 && 'service_id' in value && typeof value.service_id === 'string' && uuid.test(value.service_id);
  return (value.type === 'complete' || value.type === 'cancel') && keys.length === 2
    && 'ticket_id' in value && typeof value.ticket_id === 'string' && uuid.test(value.ticket_id);
}

const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

// One unresolved intention per queue and browser tab. Loading never sends it.
export class QueueCommands {
  readonly scope: CommandScope;
  readonly storageKey: string;
  pending: PendingCommand | null = null;
  storageError: unknown = null;
  busy = false;
  private storage: JournalStorage | null;
  private send: SendCommand;

  constructor(scope: CommandScope, storage: JournalStorage | null, send: SendCommand = (s, p) => api.queueCommand(s.tenant, s.queue, p.key, p.command)) {
    this.scope = scope;
    this.storage = storage;
    this.send = send;
    this.storageKey = `turnesta:queue-command:v1:${[scope.tenant, scope.queue, scope.user].map(encodeURIComponent).join(':')}`;
    try {
      if (!storage) throw new Error('Command storage unavailable');
      const raw = storage.getItem(this.storageKey);
      if (raw !== null) {
        const value: unknown = JSON.parse(raw);
        if (!value || typeof value !== 'object' || !('key' in value) || typeof value.key !== 'string' || !uuid.test(value.key)
          || !('command' in value) || !validCommand(value.command)
          || !('created_at' in value) || typeof value.created_at !== 'string' || !Number.isFinite(Date.parse(value.created_at))) {
          throw new Error('Invalid saved queue command');
        }
        this.pending = { key: value.key, command: value.command, created_at: value.created_at };
      }
    } catch (error) { this.storageError = error; }
  }

  private clear() {
    try { this.storage!.removeItem(this.storageKey); this.pending = null; }
    catch (error) { this.storageError = error; throw error; }
  }

  async run(command?: QueueCommand): Promise<QueueCommandResult> {
    if (this.storageError) throw this.storageError;
    if (this.busy || (command && this.pending) || (!command && !this.pending)) throw new Error('Queue command is already unresolved');
    const recovering = this.pending !== null;
    this.busy = true;
    try {
      if (command) {
        const pending: PendingCommand = { key: crypto.randomUUID(), command: { ...command }, created_at: new Date().toISOString() };
        try { this.storage!.setItem(this.storageKey, JSON.stringify(pending)); }
        catch (error) { this.storageError = error; throw error; }
        this.pending = pending;
      }
      let result: QueueCommandResult;
      try { result = await this.send(this.scope, this.pending!); }
      catch (error) {
        // Authentication can be restored; network/5xx/malformed responses do not
        // prove rollback. Retain the original key and payload for explicit retry.
        if (error instanceof ApiError && error.status >= 400 && error.status < 500 && error.status !== 401
          && error.code !== 'idempotency_conflict'
          && (!recovering || ['state_conflict', 'active_ticket', 'service_unavailable'].includes(error.code))) this.clear();
        throw error;
      }
      this.clear();
      return result;
    } finally { this.busy = false; }
  }
}
