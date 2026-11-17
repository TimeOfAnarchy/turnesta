import { expect, test } from '@playwright/test';

const tenant = '15f84bd5-0dfa-43b9-8fd9-171f526bad37';
const user = '1640756a-6d27-4ff1-b88a-ab4cbad8817a';
const point = 'e0c6c142-f8d4-42d1-a6db-6f84719fa843';
const specialistId = '0974dc70-c887-4aa2-95a1-071b1967e08f';
const workspacePath = `/app/organizations/${tenant}`;
const list = items => ({ items, next_cursor: null });

async function workspace(page) {
  const state = {
    organization: { id: tenant, name: 'Клиника', version: 1, visible_services: ['queues', 'bookings'], booking_retention_months: 24 },
    specialist: { id: specialistId, name: 'Доктор', description: '', enabled: true, version: 1 },
  };
  await page.clock.install();
  await page.route('**/auth/session', route => route.fulfill({ json: { user_id: user } }));
  await page.route('**/api/organizations**', route => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/api/organizations') return route.fulfill({ json: list([{ ...state.organization, role: 'owner' }]) });
    if (path === `/api/organizations/${tenant}`) return route.fulfill({ json: state.organization });
    if (path.endsWith('/locations')) return route.fulfill({ json: list([{ id: point, tenant_id: tenant, name: 'Клиника', time_zone: 'UTC', version: 1 }]) });
    if (path.endsWith('/specialists')) return route.fulfill({ json: list([state.specialist]) });
    if (path.endsWith('/presets')) return route.fulfill({ json: list([{ id: user, name: 'Приём', duration_minutes: 30, enabled: true, version: 1 }]) });
    return route.fulfill({ json: list([]) });
  });
  return state;
}

test('Settings preserve drafts during polling and adopt fresh data after an explicit reset', async ({ page }) => {
  const state = await workspace(page);
  await page.goto(`${workspacePath}/settings`);
  const name = page.getByLabel('Название организации');
  const months = page.getByLabel('Срок в месяцах');
  const queues = page.getByLabel('Показывать «Очереди»');
  await name.fill('Мой черновик');
  await months.fill('6');
  await queues.uncheck();
  state.organization = { ...state.organization, version: 2, name: 'Другое название', visible_services: ['tips'], booking_retention_months: 12 };
  await page.clock.fastForward(15_000);
  await expect(page.getByRole('button', { name: 'Загрузить текущую версию' })).toBeVisible();
  await expect(name).toHaveValue('Мой черновик');
  await expect(months).toHaveValue('6');
  await expect(queues).not.toBeChecked();
  await expect(page.getByLabel('Показывать «Запись»')).toBeChecked();
  await page.getByRole('button', { name: 'Загрузить текущую версию' }).click();
  await page.getByRole('button', { name: 'Использовать текущее название' }).click();
  await page.getByRole('button', { name: 'Загрузить текущие настройки' }).click();
  await page.getByRole('button', { name: 'Использовать сохранённый выбор' }).click();
  await page.getByRole('button', { name: 'Использовать текущие значения' }).click();
  await expect(name).toHaveValue('Другое название');
  await expect(months).toHaveValue('12');
  await expect(page.getByLabel('Показывать «Чаевые»')).toBeChecked();
  state.organization = { ...state.organization, version: 3, name: 'Новые данные', visible_services: ['queues'], booking_retention_months: 36 };
  await page.clock.fastForward(15_000);
  await expect(name).toHaveValue('Новые данные');
  await expect(months).toHaveValue('36');
  await expect(queues).toBeChecked();
});

test('A changed specialist cancels the batch preview, preserves entered times and accepts a new preview', async ({ page }) => {
  const state = await workspace(page);
  const held = [];
  await page.route('**/bookings/batch-preview', route => { held.push(route); });
  await page.goto(`${workspacePath}/bookings`);
  await page.getByRole('button', { name: 'Заполнить на несколько дней', exact: true }).first().click();
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('Начало периода').fill('09:00');
  await dialog.getByLabel('Конец периода').fill('10:00');
  const preview = dialog.getByRole('button', { name: 'Предпросмотр', exact: true });
  await preview.click();
  await expect.poll(() => held.length).toBe(1);
  state.specialist = { ...state.specialist, name: 'Доктор — обновлён', version: 2 };
  await page.clock.fastForward(10_000);
  await expect(dialog).toContainText('Доктор — обновлён');
  await expect(preview).toBeEnabled();
  await expect(dialog.getByLabel('Начало периода')).toHaveValue('09:00');
  await expect(dialog.getByLabel('Конец периода')).toHaveValue('10:00');
  await preview.click();
  await expect.poll(() => held.length).toBe(2);
  const response = (route, title) => {
    const body = route.request().postDataJSON();
    const date = body.source.dates[0];
    return { token: 'a'.repeat(64), rows: [{ index: 1, title, mode: 'interval', date,
      starts_local: `${date}T09:00`, ends_local: `${date}T09:30`, starts_at: `${date}T09:00:00Z`,
      ends_at: `${date}T09:30:00Z`, time_zone: 'UTC', places: 1, state: 'ready' }],
      conflicts: [], conflicts_truncated: false, has_duplicates: false };
  };
  await held[1].fulfill({ json: response(held[1], 'Новый предпросмотр') });
  await expect(dialog.getByText('Новый предпросмотр', { exact: true })).toBeVisible();
  await held[0].fulfill({ json: response(held[0], 'Старый предпросмотр') });
  await expect(dialog.getByText('Старый предпросмотр', { exact: true })).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: 'Добавить талоны · 1' })).toBeEnabled();
});

test('The resend button becomes available when its cooldown expires', async ({ page }) => {
  await workspace(page);
  const now = Date.now();
  await page.clock.setFixedTime(now);
  await page.route('**/team?*', route => route.fulfill({
    json: list([{ user_id: user, display_name: 'Сотрудник', email: 'staff@example.test',
      role: 'operator', active: true, version: 1, delivery_state: 'deferred', delivery_attempts: null,
      delivery_expires_at: null, resend_after: new Date(now + 2000).toISOString() }]),
  }));
  await page.goto(`${workspacePath}/team`);
  const resend = page.getByRole('button', { name: 'Отправить письмо для входа', exact: true });
  await expect(resend).toBeDisabled();
  await page.clock.setFixedTime(now + 3000);
  await page.clock.runFor(1000);
  await expect(resend).toBeEnabled();
});
