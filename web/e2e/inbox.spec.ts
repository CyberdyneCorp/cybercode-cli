import { test, expect, login, select, tab } from './browser.js';
import { DashboardFixture, eventually } from './fixture.js';

test('held agent content stays inert and never reaches a model before explicit browser consent', async ({ page }, info) => {
  const dashboard = await DashboardFixture.create({ messaging: { inbound: { default: 'hold' } } });
  try {
    await dashboard.start(); const sender = await dashboard.session('Sender'); const receiver = await dashboard.session('Receiver inbox');
    const result = await dashboard.api<{ outcome: string }>('/messaging/send', 'POST', { id: 'msgx_browser_held', from: sender, to: receiver, text: 'HOSTILE_BROWSER <img src="https://dashboard-beacon.invalid/pixel"> <script>globalThis.__inboxPwned=true</script>' });
    expect(result.outcome).toBe('held'); expect(dashboard.modelRequests.length).toBe(0);
    const external: string[] = []; page.on('request', (request) => { if (request.url().includes('dashboard-beacon.invalid')) external.push(request.url()); });
    await login(page, dashboard); await select(page, 'Receiver inbox'); await tab(page, 'Message inbox');
    const card = page.getByRole('article', { name: 'Held message msgx_browser_held', exact: true });
    await expect(card).toContainText('HOSTILE_BROWSER'); await expect(card).toContainText(receiver);
    await expect(card.getByRole('button', { name: 'Release to queue', exact: true })).toBeDisabled();
    expect(await page.evaluate(() => '__inboxPwned' in globalThis)).toBe(false); expect(external).toEqual([]); expect(dashboard.modelRequests.length).toBe(0);
    await page.setViewportSize({ width: 360, height: 800 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(360);
    await page.screenshot({ path: info.outputPath('message-inbox-360.png'), fullPage: true });
    await card.getByRole('checkbox', { name: 'I reviewed this sender, target and message content' }).check();
    await card.getByRole('button', { name: 'Release to queue', exact: true }).click();
    await expect(card).toHaveCount(0); await eventually(async () => dashboard.modelRequests.length, (count) => count === 1);
    expect((await dashboard.api<{ status: string }[]>(`/sessions/${receiver}/inbox`)).some((row) => row.status === 'held')).toBe(false);
  } finally { await dashboard.close(); }
});

test('uncertain held-message release retries only the original message, payload and key', async ({ page }) => {
  const dashboard = await DashboardFixture.create({ messaging: { inbound: { default: 'hold' } } });
  try {
    await dashboard.start(); const sender = await dashboard.session('Sender'); const receiver = await dashboard.session('Receiver retry');
    await dashboard.api('/messaging/send', 'POST', { id: 'msgx_browser_retry', from: sender, to: receiver, text: 'HOSTILE_BROWSER retry' });
    await login(page, dashboard); await select(page, 'Receiver retry'); await tab(page, 'Message inbox');
    const requests: { key: string | undefined; body: string | null; url: string }[] = [];
    await page.route(`**/sessions/${receiver}/inbox/msgx_browser_retry/release`, async (route) => {
      const request = route.request(); requests.push({ key: request.headers()['idempotency-key'], body: request.postData(), url: request.url() });
      if (requests.length === 1) await route.abort('connectionreset'); else await route.continue();
    });
    const card = page.getByRole('article', { name: 'Held message msgx_browser_retry', exact: true });
    await card.getByRole('checkbox').check(); await card.getByRole('button', { name: 'Release to queue', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Retry message decision', exact: true })).toBeVisible(); expect(dashboard.modelRequests.length).toBe(0);
    await page.getByRole('button', { name: 'Refresh message inbox', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Retry message decision', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Retry message decision', exact: true }).click();
    await expect(card).toHaveCount(0); await eventually(async () => dashboard.modelRequests.length, (count) => count === 1);
    expect(requests.length).toBe(2); expect(Boolean(requests[0]?.key)).toBe(true); expect(requests[1]).toEqual(requests[0]);
  } finally { await dashboard.close(); }
});

test('another owner client can drop a held message without browser release or inference', async ({ page }) => {
  const dashboard = await DashboardFixture.create({ messaging: { inbound: { default: 'hold' } } });
  try {
    await dashboard.start(); const sender = await dashboard.session('Sender'); const receiver = await dashboard.session('Receiver race');
    await dashboard.api('/messaging/send', 'POST', { id: 'msgx_browser_dropped', from: sender, to: receiver, text: 'HOSTILE_BROWSER dropped' });
    await login(page, dashboard); await select(page, 'Receiver race'); await tab(page, 'Message inbox');
    const card = page.getByRole('article', { name: 'Held message msgx_browser_dropped', exact: true }); await expect(card).toBeVisible();
    await dashboard.api(`/sessions/${receiver}/inbox/msgx_browser_dropped/drop`, 'POST');
    await page.getByRole('button', { name: 'Refresh message inbox', exact: true }).click(); await expect(card).toHaveCount(0);
    expect(dashboard.modelRequests.length).toBe(0);
  } finally { await dashboard.close(); }
});
