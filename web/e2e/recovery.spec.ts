import { test, expect, login, select, send, tab } from './browser.js';
import { DashboardFixture, eventually } from './fixture.js';

test('offline cards cannot submit, reconnect refetches live pending state', async ({ dashboard, page, context }) => {
  const session = await dashboard.session('Reconnect acceptance');
  await login(page, dashboard); await select(page, 'Reconnect acceptance');
  await send(page, 'WRITE_BROWSER:reconnect');
  const request = await dashboard.waitPending('permissions', session);
  await tab(page, 'Approvals');
  const approval = page.getByRole('article', { name: `Permission request ${request.id}` }).getByRole('button', { name: 'Approve once', exact: true });
  await expect(approval).toBeVisible();
  await context.setOffline(true);
  await expect(page.getByRole('status', { name: 'Connection status' })).toContainText(/offline|reconnecting/iu);
  // A disconnected view may retain a disabled card or remove the stale card.
  await expect.poll(async () => await approval.count() === 0 || await approval.isDisabled()).toBe(true);
  await dashboard.api(`/sessions/${session}/permissions/${request.id}/reply`, 'POST', { reply: 'reject', message: 'Terminal answered while browser was offline' });
  await context.setOffline(false);
  await expect(page.getByRole('status', { name: 'Connection status' })).toContainText(/connected/iu);
  await expect(page.getByRole('article', { name: `Permission request ${request.id}` })).toHaveCount(0);
  expect(await dashboard.file('reconnect.txt')).toBeUndefined();
});

test('server restart invalidates the browser lease and requires fresh attachment', async ({ dashboard, page }) => {
  await dashboard.session('Restart acceptance');
  const leaseIds: string[] = [];
  page.on('response', async (response) => {
    if (response.url().endsWith('/interactive-clients') && response.request().method() === 'POST' && response.ok()) {
      const body = await response.json() as { data: { client_id: string } }; leaseIds.push(body.data.client_id);
    }
  });
  await login(page, dashboard); await select(page, 'Restart acceptance');
  await expect.poll(() => leaseIds.length).toBe(1);
  const previous = (await dashboard.api<{ id: string }>('/health')).id;
  await dashboard.restart();
  expect((await dashboard.api<{ id: string }>('/health')).id === previous).toBe(false);
  // A restarted owner process must not inherit its predecessor's interactive proof.
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible({ timeout: 30_000 });
  await login(page, dashboard); await select(page, 'Restart acceptance');
  await expect.poll(() => leaseIds.length).toBeGreaterThanOrEqual(2);
  expect(new Set(leaseIds).size).toBeGreaterThanOrEqual(2);
});

test('a real 45-second browser lease expires without silently approving work', async ({ dashboard, page }) => {
  test.setTimeout(85_000);
  const session = await dashboard.session('Expiry acceptance');
  await login(page, dashboard); await select(page, 'Expiry acceptance');
  let lease: { client_id: string; expires_at: number } | undefined;
  // Reload creates a known fresh attachment; retain no password across the reload.
  await page.reload();
  page.on('response', async (response) => {
    if (response.url().endsWith('/interactive-clients') && response.request().method() === 'POST' && response.ok()) {
      lease = (await response.json() as { data: { client_id: string; expires_at: number } }).data;
    }
  });
  await login(page, dashboard); await select(page, 'Expiry acceptance');
  await expect.poll(() => Boolean(lease)).toBe(true);
  await page.route('**/interactive-clients', async (route) => { if(route.request().method()==='POST')await route.abort('connectionreset');else await route.continue(); });
  await page.route('**/interactive-clients/*', async (route) => {
    if (route.request().method() === 'PUT') await route.abort('connectionreset'); else await route.continue();
  });
  await dashboard.prompt(session, 'WRITE_BROWSER:waiting_expiry');
  await dashboard.waitPending('permissions',session);
  expect(await dashboard.file('waiting_expiry.txt')).toBeUndefined();
  await page.waitForTimeout(Math.max(0, lease!.expires_at - Date.now()) + 1_500);
  const renewal = await dashboard.request(`/sessions/${session}/interactive-clients/${lease!.client_id}`, 'PUT', {});
  expect(renewal.status).toBe(404);
  await eventually(()=>dashboard.pending('permissions',session),(pending)=>pending.length===0);
  expect(await dashboard.file('waiting_expiry.txt')).toBeUndefined();
  await dashboard.prompt(session, 'WRITE_BROWSER:expired_lease');
  await eventually(() => dashboard.api<{ status: string }>(`/sessions/${session}`), (state) => state.status === 'idle');
  expect((await dashboard.pending('permissions', session)).length).toBe(0);
  expect(await dashboard.file('expired_lease.txt')).toBeUndefined();
});

test('an expiring browser lease does not clear an independently attached terminal', async ({ dashboard, page }) => {
  test.setTimeout(85_000);
  const session = await dashboard.session('Terminal presence');
  let leaseId:string|undefined;
  page.on('response',async response=>{if(response.url().endsWith('/interactive-clients')&&response.request().method()==='POST'&&response.ok()){leaseId=(await response.json() as {data:{client_id:string}}).data.client_id;}});
  await login(page, dashboard); await select(page, 'Terminal presence');
  await expect.poll(()=>Boolean(leaseId)).toBe(true);
  const controller = new AbortController();
  const stream = await fetch(`${dashboard.url}/api/v1/sessions/${session}/events`, { headers: { authorization: dashboard.authorization, 'x-cyber-directory': dashboard.repo }, signal: controller.signal });
  expect(stream.ok).toBe(true);
  const reader = stream.body!.getReader();
  const consuming = (async () => { try { while (!(await reader.read()).done) { /* keep this independent terminal observer attached */ } } catch { /* explicit cleanup abort */ } })();
  try {
    await page.route('**/interactive-clients',async route=>{if(route.request().method()==='POST')await route.abort('connectionreset');else await route.continue();});
    await page.route('**/interactive-clients/*', async (route) => { if (route.request().method() === 'PUT') await route.abort('connectionreset'); else await route.continue(); });
    await page.waitForTimeout(46_500);
    expect((await dashboard.request(`/sessions/${session}/interactive-clients/${leaseId!}`,'PUT',{})).status).toBe(404);
    await dashboard.prompt(session, 'WRITE_BROWSER:terminal_remains');
    const pending = await dashboard.waitPending('permissions', session);
    expect(await dashboard.file('terminal_remains.txt')).toBeUndefined();
    const reply = await dashboard.terminal(['api', 'POST', `/sessions/${session}/permissions/${pending.id}/reply`, '-d', JSON.stringify({ reply: 'reject' })]);
    expect(reply.code).toBe(0);
  } finally { controller.abort(); await consuming; }
});

test('SSH-style loopback TCP forwarding supports the real browser approval flow', async ({ dashboard, page }) => {
  const session = await dashboard.session('Forwarded approval');
  const forwarding = await dashboard.forward();
  try {
    await login(page, dashboard, forwarding.url); await select(page, 'Forwarded approval');
    await send(page, 'WRITE_BROWSER:forwarded');
    const pending = await dashboard.waitPending('permissions', session);
    await tab(page, 'Approvals');
    await page.getByRole('article', { name: `Permission request ${pending.id}` }).getByRole('button', { name: 'Approve once', exact: true }).click();
    await eventually(() => dashboard.file('forwarded.txt'), Boolean);
    expect(new URL(page.url()).origin).toBe(forwarding.url);
  } finally { await forwarding.close(); }
});

test('non-loopback plaintext web binding is refused before starting a listener', async () => {
  const dashboard = await DashboardFixture.create();
  try {
    const result = await dashboard.terminal(['web', '--hostname', '0.0.0.0', '--port', String(dashboard.port), '--no-open']);
    expect(result.code).toBe(2);
    expect(result.stderr).toMatch(/requires.*server\.tls/u);
    expect((result.stdout + result.stderr).includes(dashboard.password)).toBe(false);
  } finally { await dashboard.close(); }
});
