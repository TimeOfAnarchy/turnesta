import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  timeout: 120_000,
  expect: { timeout: 10_000 },
  workers: 1,
  forbidOnly: Boolean(process.env.CI),
  use: {
    baseURL: process.env.E2E_BASE_URL || 'http://127.0.0.1:8080',
    locale: 'ru-RU',
    timezoneId: 'UTC',
    viewport: { width: 1280, height: 800 },
  },
  projects: ['chromium', 'firefox', 'webkit'].map(browserName => ({
    name: browserName,
    use: { browserName },
  })),
});
