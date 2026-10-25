import { test as base, expect, type Page } from '@playwright/test';
import { DashboardFixture } from './fixture.js';

export const test = base.extend<{ dashboard: DashboardFixture }>({
  dashboard: async ({}, use, info) => {
    const dashboard = await DashboardFixture.create();
    try { await dashboard.start(); await use(dashboard); }
    finally {
      if (info.status !== info.expectedStatus) await info.attach('server-log-redacted', { body: dashboard.logs(), contentType: 'text/plain' });
      await dashboard.close();
    }
  },
});
export { expect };
export type Lease = { client_id: string; expires_at: number };
/** Record each browser lease grant. The body is read on the Node side: a page
 * 'response' listener can find it already evicted and throw "No data found". */
export async function recordLeases(page: Page): Promise<Lease[]> {
  const leases: Lease[] = [];
  await page.route('**/interactive-clients', async (route) => {
    if (route.request().method() !== 'POST') return route.fallback();
    const response = await route.fetch().catch(() => undefined);
    if (!response) return route.abort('connectionrefused');
    if (response.ok()) leases.push((await response.json() as { data: Lease }).data);
    await route.fulfill({ response });
  });
  return leases;
}
export async function login(page: Page, dashboard: DashboardFixture, origin = dashboard.url): Promise<void> {
  await page.goto(origin);
  await expect(page.getByLabel('Password', { exact: true })).toBeVisible();
  // Avoid including even this isolated test password in Playwright's fill call log.
  await page.evaluate((password) => {
    const input = document.querySelector<HTMLInputElement>('input[type="password"]');
    if (!input) throw new Error('Password input is unavailable');
    input.value = password;
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
  }, dashboard.password);
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Logout', exact: true })).toBeVisible();
}
export async function select(page: Page, title: string): Promise<void> {
  const session=page.getByRole('button', { name: title, exact: true });
  await expect(session).toBeVisible();
  if(await session.getAttribute('aria-pressed')!=='true'&&await session.getAttribute('aria-current')!=='page')await session.click();
  await expect(page.getByRole('status', { name: 'Connection status' })).toContainText(/connected/iu);
}
export async function tab(page: Page, label: string): Promise<void> {
  const button = page.getByRole('tab', { name: new RegExp(label, 'iu') });
  if (await button.count()) await button.click();
  else await page.getByRole('button', { name: new RegExp(label, 'iu') }).click();
}
export async function send(page: Page, text: string): Promise<void> {
  await tab(page, 'Conversation');
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill(text);
  await page.getByRole('button', { name: 'Send message', exact: true }).click();
}
