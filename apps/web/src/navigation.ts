import type { Role, WorkspaceService } from './api';

export const serviceOptions = [
  { id: 'queues', label: 'Очереди', icon: 'queue', description: 'Талоны, вызов посетителей и табло.' },
  { id: 'bookings', label: 'Запись', icon: 'calendar', description: 'Расписание, клиенты и онлайн-запись на сайте.' },
  { id: 'tips', label: 'Чаевые', icon: 'heart', description: 'Сейчас доступен только тестовый режим оплаты.' },
] as const;

const sections = ['locations', 'queues', 'bookings', 'tips', 'team', 'settings'];
export function sectionService(section: string): WorkspaceService | undefined {
  return serviceOptions.find(service => service.id === section)?.id;
}
export function startSection(saved: string | null, visible: readonly WorkspaceService[], role: Role): string {
  const service = sectionService(saved ?? '');
  if (saved && sections.includes(saved) && (saved !== 'team' || role === 'owner') && (!service || visible.includes(service))) return saved;
  return visible.includes('bookings') ? 'bookings' : 'locations';
}
export function sameServices(a: readonly WorkspaceService[], b: readonly WorkspaceService[]) {
  return a.length === b.length && a.every(service => b.includes(service));
}
