import { expect } from '@playwright/test';
import { checkAccessibility, checkWidth, mobile, test } from '../fixtures.js';

const visit = 'Первичный приём';
const tabs = page => page.getByRole('navigation', { name: 'Разделы записи' });

async function chooseVisit(view, date) {
  await expect(view.getByRole('heading', { name: 'Выберите время' })).toBeVisible();
  const month = new Date(date + 'T12:00:00Z').toLocaleDateString('ru-RU', {
    month: 'long', year: 'numeric', timeZone: 'UTC',
  });
  if (!(await view.getByText(month, { exact: true }).count())) {
    await view.getByRole('button', { name: 'Следующий месяц' }).click();
  }
  await view.getByRole('button', { name: /есть талоны/ }).click();
  await view.getByRole('button', { name: new RegExp(visit) }).click();
  await view.getByRole('button', { name: 'Продолжить' }).click();
  await expect(view.getByRole('heading', { name: 'Проверьте запись' })).toBeVisible();
}

async function confirmVisit(view, name, phone) {
  await view.getByRole('textbox', { name: /имя/i }).fill(name);
  await view.getByRole('textbox', { name: /телефон/i }).fill(phone);
  await view.getByRole('checkbox').check();
  await view.getByRole('button', { name: 'Подтвердить запись' }).click();
  await expect(view.getByRole('heading', { name: 'Вы записаны' })).toBeVisible();
}

test('Вход, расписание, онлайн-запись, отмена и виджет клиники', async ({ page, publicPage, clinicPage, owner, clinic, browserName }) => {
  const dialog = page.getByRole('dialog');
  let date;
  let publicUrl;
  let confirmationNumber;

  await test.step('Первый вход и повторный вход с постоянным паролем', async () => {
    await page.goto('/login');
    await checkAccessibility(page, 'login');
    await page.getByLabel('Email', { exact: true }).fill(owner.email);
    await page.getByLabel('Пароль', { exact: true }).fill(owner.temporary);
    await page.getByRole('button', { name: 'Войти', exact: true }).click();
    const setup = page.getByRole('region', { name: 'Задайте свой пароль' });
    await setup.getByLabel('Новый пароль', { exact: true }).fill(owner.password);
    await setup.getByLabel('Повторите новый пароль').fill(owner.password);
    await page.getByRole('button', { name: 'Сохранить пароль и войти' }).click();
    await page.getByRole('button', { name: 'Выйти из Turnesta' }).click();
    await expect(page).toHaveURL(/\/login$/);
    await page.getByLabel('Email', { exact: true }).fill(owner.email);
    await page.getByLabel('Пароль', { exact: true }).fill(owner.password);
    await page.getByRole('button', { name: 'Войти', exact: true }).click();
    await expect(page).toHaveURL(/\/app\//);
  });

  await test.step('Владелец создаёт точку, специалиста и талон на завтра', async () => {
    await page.getByRole('link', { name: 'Точки', exact: true }).click();
    await page.getByRole('button', { name: 'Добавить точку', exact: true }).click();
    await dialog.getByLabel('Название точки').fill('Клиника на Садовой');
    await dialog.getByLabel('Часовой пояс').fill('UTC');
    await dialog.getByRole('button', { name: 'Добавить точку' }).click();
    await page.getByRole('link', { name: 'Настроить запись к специалистам' }).click();
    await page.getByRole('button', { name: 'Добавить специалиста' }).click();
    await dialog.getByLabel('Имя специалиста').fill('Доктор Соколова');
    await dialog.getByRole('button', { name: 'Сохранить', exact: true }).click();
    const schedule = page.getByLabel('Дата расписания');
    const tomorrow = new Date((await schedule.inputValue()) + 'T12:00:00Z');
    tomorrow.setUTCDate(tomorrow.getUTCDate() + 1);
    date = tomorrow.toISOString().slice(0, 10);
    await schedule.fill(date);
    await page.getByRole('button', { name: 'Добавить талон' }).click();
    await dialog.getByLabel('Название приёма', { exact: false }).fill(visit);
    await dialog.getByLabel('Начало', { exact: true }).fill('14:00');
    await dialog.getByLabel('Окончание', { exact: true }).fill('14:30');
    await dialog.getByRole('button', { name: 'Сохранить талон' }).click();
    await expect(page.getByText(visit, { exact: true })).toBeVisible();
    await checkAccessibility(page, 'schedule');
  });

  await test.step('Публикация ссылки и кода виджета для другого origin', async () => {
    await tabs(page).getByRole('link', { name: /Онлайн-запись/ }).click();
    await page.getByLabel('Ссылка на политику обработки персональных данных').fill('https://clinic.example.test/privacy');
    await page.getByLabel('Принимать записи через публичную страницу').check();
    await page.getByRole('button', { name: 'Сохранить настройки записи' }).click();
    const link = page.getByRole('link', { name: /\/book\// });
    await expect(link).toBeVisible();
    publicUrl = await link.getAttribute('href');
    expect(new URL(publicUrl).pathname).toMatch(/^\/book\/[^/]+$/);
    expect(new URL(publicUrl).origin).not.toBe(clinic.origin);
    await page.getByLabel(/Разрешённые сайты/).fill(clinic.origin);
    await page.getByRole('button', { name: 'Сохранить сайты' }).click();
    const code = page.getByLabel('Код для сайта');
    await expect(code).toBeVisible();
    clinic.publish(await code.inputValue());
  });

  await test.step('Посетитель записывается на публичной странице при ширине 390 px', async () => {
    await publicPage.goto(publicUrl);
    await chooseVisit(publicPage, date);
    await checkWidth(publicPage);
    await checkAccessibility(publicPage, 'public booking form');
    await confirmVisit(publicPage, 'Анна Тестовая', '+79000001122');
    confirmationNumber = await publicPage.getByLabel('Номер записи', { exact: true }).inputValue();
    expect(confirmationNumber).toMatch(/^\d{6,}$/);
    // Success feedback only after the browser accepts the clipboard write.
    if (browserName === 'chromium') {
      await publicPage.context().grantPermissions(['clipboard-read', 'clipboard-write']);
    } else {
      await publicPage.evaluate(() => Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async value => { window.copiedNumber = value; } } }));
    }
    await publicPage.getByRole('button', { name: 'Копировать номер' }).click();
    await expect(publicPage.getByRole('status')).toHaveText('Номер скопирован.');
    const copied = browserName === 'chromium'
      ? await publicPage.evaluate(() => navigator.clipboard.readText())
      : await publicPage.evaluate(() => window.copiedNumber);
    expect(copied).toBe(confirmationNumber);
    await checkAccessibility(publicPage, 'public booking receipt');
    await checkWidth(publicPage);
  });

  await test.step('Кабинет показывает запись, отмена освобождает талон', async () => {
    await tabs(page).getByRole('link', { name: /Расписание/ }).click();
    await page.getByLabel('Дата расписания').fill(date);
    await page.getByRole('button', { name: /Открыть запись Анна Тестовая/ }).click();
    await expect(dialog.getByLabel('Номер записи', { exact: true })).toHaveValue(confirmationNumber);
    await page.evaluate(() => Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => { throw new Error('Clipboard denied'); } } }));
    await dialog.getByRole('button', { name: 'Копировать номер' }).click();
    await expect(dialog.getByRole('status')).toHaveText('Номер выделен. Скопируйте его через меню устройства.');
    await expect(dialog.getByLabel('Номер записи', { exact: true })).toBeFocused();
    await dialog.getByRole('button', { name: 'Отменить запись' }).click();
    await dialog.getByRole('button', { name: 'Да, отменить запись' }).click();
    await expect(page.getByText('Запись отменена. Место освобождено.', { exact: true })).toBeVisible();
    await publicPage.getByRole('button', { name: 'Обновить запись' }).click();
    await expect(publicPage.getByRole('heading', { name: 'Запись отменена' })).toBeVisible();
    await expect(publicPage.getByLabel('Номер записи', { exact: true })).toHaveValue(confirmationNumber);
    await page.setViewportSize(mobile);
    await checkWidth(page);
  });

  await test.step('Виджет на сайте клиники записывает второго посетителя', async () => {
    await clinicPage.goto(clinic.origin);
    const iframe = clinicPage.getByTitle('Онлайн-запись', { exact: true });
    await expect(iframe).toBeVisible();
    const status = clinicPage.getByRole('status', { includeHidden: true });
    await expect(status).toHaveCount(1);
    await expect(status).toBeHidden();
    const widget = iframe.contentFrame();
    await chooseVisit(widget, date);
    await checkWidth(clinicPage);
    await checkWidth(widget);
    await checkAccessibility(clinicPage, 'clinic and embedded booking form');
    await confirmVisit(widget, 'Мария Тестовая', '+79000003344');
    const widgetNumber = widget.getByLabel('Номер записи', { exact: true });
    await expect(widgetNumber).toHaveValue(/^[0-9]{6,}$/);
    expect(await widgetNumber.inputValue()).not.toBe(confirmationNumber);
    await widget.locator('html').evaluate(() => Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => { throw new Error('Clipboard denied'); } } }));
    await widget.getByRole('button', { name: 'Копировать номер' }).click();
    await expect(widget.getByRole('status')).toHaveText('Номер выделен. Скопируйте его через меню устройства.');
    await expect(widgetNumber).toBeFocused();
    const selectedNumber = await widgetNumber.evaluate(el => el.value.slice(el.selectionStart, el.selectionEnd));
    expect(selectedNumber).toBe(await widgetNumber.inputValue());
    await checkWidth(widget);
    await page.reload();
    await page.getByLabel('Дата расписания').fill(date);
    await expect(page.getByRole('button', { name: /Открыть запись Мария Тестовая/ })).toBeVisible();
  });
});
