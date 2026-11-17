import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { createServer } from 'node:http';
import { fileURLToPath } from 'node:url';
import { expect, test as base } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

const repository = fileURLToPath(new URL('../', import.meta.url));

function admin(args, password) {
  const project = process.env.COMPOSE_PROJECT_NAME;
  if (!project) throw new Error('Set COMPOSE_PROJECT_NAME to the dedicated E2E stack.');
  return JSON.parse(execFileSync('docker', [
    'compose', '--project-name', project, 'run', '--rm', '--no-deps', '-T', 'admin', ...args,
  ], { cwd: repository, input: password, encoding: 'utf8' }));
}

export const mobile = { width: 390, height: 844 };

async function visitorPage({ browser }, use) {
  const context = await browser.newContext({ viewport: mobile, locale: 'ru-RU', timezoneId: 'UTC' });
  try {
    await use(await context.newPage());
  } finally {
    await context.close();
  }
}

export const test = base.extend({
  publicPage: visitorPage,
  clinicPage: visitorPage,
  owner: async ({ browserName }, use) => {
    const email = `owner-${randomUUID()}@example.test`;
    const temporary = randomUUID();
    const { owner } = admin(['core', 'organization-create', '--name', `Клиника ${browserName}`, '--owner-name', 'Тестовый владелец']);
    admin(['auth', 'password-set', '--user', owner.id, '--email', email, '--password-stdin'], temporary);
    await use({ email, temporary, password: randomUUID() });
  },

  clinic: async ({}, use) => {
    let snippet = '';
    const server = createServer((request, response) => {
      if (request.url !== '/' || !snippet) {
        response.writeHead(404).end();
        return;
      }
      response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
      response.end(`<!doctype html><html lang="ru"><head><meta charset="utf-8">
        <meta name="viewport" content="width=device-width, initial-scale=1"><title>Тестовая клиника</title>
        </head><body><main><h1>Запись в клинику</h1>${snippet}</main></body></html>`);
    });
    await new Promise((resolve, reject) => server.once('error', reject).listen(0, '127.0.0.1', resolve));
    try {
      await use({
        origin: `http://127.0.0.1:${server.address().port}`,
        publish(code) { snippet = code; },
      });
    } finally {
      server.closeAllConnections();
      await new Promise(resolve => server.close(resolve));
    }
  },
});

export async function checkAccessibility(page, surface) {
  const { violations, incomplete } = await new AxeBuilder({ page }).analyze();
  const issues = violations.filter(v => ['serious', 'critical'].includes(v.impact))
    .map(v => ({ rule: v.id, impact: v.impact, nodes: v.nodes.map(n => n.target) }));
  expect(incomplete.filter(v => v.id === 'frame-tested'), `axe could not inspect a frame on ${surface}`).toEqual([]);
  expect(issues, `Accessibility of ${surface}`).toEqual([]);
}

export async function checkWidth(document) {
  await expect.poll(() => document.locator('html').evaluate(el =>
    el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(0);
}
