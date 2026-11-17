import { expect, test } from '@playwright/test';
import { checkAccessibility, checkWidth, mobile } from '../fixtures.js';

test('Render failure shows recovery and preserves a pending command through reload', async ({ page }, testInfo) => {
  const user = 'f226790a-30cd-4a11-bfff-4d1b8c19c7bc';
  const tenant = '3e1b2f28-cb62-479a-b88c-583bf38f1f17';
  const storageKey = `turnesta:team-command:v1:${tenant}:${user}`;
  const pending = JSON.stringify({
    key: 'fe23c988-6c39-437c-876e-90c4e1b1d025',
    command: { type: 'send_login', user_id: user, expected_version: 1 },
  });
  let broken = true;
  await page.route('**/auth/session', route => route.fulfill({ json: { user_id: user } }));
  await page.route('**/api/organizations?*', route => route.fulfill({
    json: { items: broken ? [null, null] : [], next_cursor: null },
  }));
  await page.goto('/health');
  await page.evaluate(({ storageKey, pending }) => sessionStorage.setItem(storageKey, pending), { storageKey, pending });
  await page.goto('/app/');
  await expect(page.getByRole('heading', { name: 'Что-то пошло не так' })).toBeVisible();
  await expect(page.getByRole('main')).not.toContainText('TypeError');
  await checkAccessibility(page, 'error recovery');
  await page.screenshot({ path: testInfo.outputPath('error-desktop.png') });
  await page.setViewportSize(mobile);
  await checkWidth(page, 'error recovery');
  await checkAccessibility(page, 'mobile error recovery');
  await page.screenshot({ path: testInfo.outputPath('error-mobile.png') });
  broken = false;
  const reload = page.getByRole('button', { name: 'Обновить страницу' });
  await page.keyboard.press('Tab');
  await expect(reload).toBeFocused();
  await Promise.all([
    page.waitForEvent('domcontentloaded'),
    page.keyboard.press('Enter'),
  ]);
  await expect(page.getByRole('heading', { name: 'Что-то пошло не так' })).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Мои организации.' })).toBeVisible();
  expect(await page.evaluate(key => sessionStorage.getItem(key), storageKey)).toBe(pending);
});
