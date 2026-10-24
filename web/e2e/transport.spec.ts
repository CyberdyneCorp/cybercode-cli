import { test, expect, login, select, send, tab } from './browser.js';
import { DashboardFixture, eventually } from './fixture.js';
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';

test('peer listener serves no dashboard and rejects the local owner password', async () => {
  const dashboard = await DashboardFixture.create({ peers: { listen: { address: '127.0.0.1:0', tls: false }, machine: 'browser-fixture' } });
  try {
    await dashboard.start();
    const peers = await dashboard.api<{ listener: { url: string } | null }>('/peers');
    expect(peers.listener !== null).toBe(true);
    const peer = peers.listener!.url;
    const shell = await fetch(`${peer}/`, { signal: AbortSignal.timeout(5_000) });
    expect(shell.status).toBe(404);
    expect((await shell.text()).includes('<html')).toBe(false);
    expect((await fetch(`${peer}/app/app.js`, { signal: AbortSignal.timeout(5_000) })).status).toBe(404);
    const owner = await fetch(`${peer}/api/v1/peer/hello`, { headers: { authorization: dashboard.authorization }, signal: AbortSignal.timeout(5_000) });
    expect(owner.status).toBe(401);
    const browserOrigin = await fetch(`${peer}/`, { headers: { origin: dashboard.url }, signal: AbortSignal.timeout(5_000) });
    expect(browserOrigin.status).toBe(403);
  } finally { await dashboard.close(); }
});

test('TLS listener serves the actual dashboard and approval API using an isolated certificate', async ({ browser }) => {
  const dashboard = await DashboardFixture.create();
  // Trust is explicit and limited to this test context. No OS CA/keychain is changed.
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
  const page = await context.newPage();
  try {
    await dashboard.configureTls();
    await dashboard.start({ hostname: '0.0.0.0', probe: async () => (await context.request.get(`${dashboard.url}/api/v1/health`)).ok() });
    const api = async (path: string, method: string, body?: unknown): Promise<unknown> => {
      const response = await context.request.fetch(`${dashboard.url}/api/v1${path}`, { method, headers: { authorization: dashboard.authorization, 'x-cyber-directory': dashboard.repo }, ...(body === undefined ? {} : { data: body }) });
      expect(response.ok()).toBe(true);
      return response.status() === 204 ? undefined : (await response.json() as { data: unknown }).data;
    };
    await api('/sessions', 'POST', { title: 'Private TLS approval', directory: dashboard.repo, model: 'test/main' });
    await login(page, dashboard); await select(page, 'Private TLS approval');
    await send(page, 'WRITE_BROWSER:tls_approved');
    await tab(page, 'Approvals');
    await expect(page.getByRole('button', { name: 'Approve once', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Approve once', exact: true }).click();
    await eventually(() => dashboard.file('tls_approved.txt'), Boolean);
    expect(new URL(page.url()).protocol).toBe('https:');
    expect((await context.request.get(`${dashboard.url}/api/v1/sessions`)).status()).toBe(401);
    const reused = await dashboard.terminal(['web', '--hostname', '0.0.0.0', '--port', String(dashboard.port), '--no-open']);
    expect(reused.code).toBe(0);
    expect(reused.stdout.includes('https://')).toBe(true);
    expect((reused.stdout + reused.stderr).includes(dashboard.password)).toBe(false);
  } finally { await context.close(); await dashboard.close(); }
});

test('project-local bogus TLS settings cannot choose the owner listener transport', async () => {
  const dashboard = await DashboardFixture.create();
  try {
    await writeFile(join(dashboard.repo, 'cyber.jsonc'), JSON.stringify({ server: { tls: { cert: 'missing-project-cert.pem', key: 'missing-project-key.pem' } } }));
    await dashboard.start();
    expect((await dashboard.request('/health')).status).toBe(200);
    const shell = await fetch(dashboard.url, { signal: AbortSignal.timeout(5_000) });
    expect(shell.status).toBe(200);
    expect(dashboard.url.startsWith('http:')).toBe(true);
  } finally { await dashboard.close(); }
});
