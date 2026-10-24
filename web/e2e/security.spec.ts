import { test, expect, login, select, send } from './browser.js';
import { eventually } from './fixture.js';

test('public shell, authenticated data, explicit memory-only credentials and inert hostile transcript', async ({ dashboard, page, context }) => {
  const session = await dashboard.session('Security acceptance');
  const shell = await page.request.get(dashboard.url);
  expect(shell.status()).toBe(200);
  const csp = shell.headers()['content-security-policy'] ?? '';
  for (const directive of ["default-src 'none'", "script-src 'self'", "style-src 'self'", "connect-src 'self'", "frame-ancestors 'none'", "base-uri 'none'", "form-action 'self'"]) expect(csp.includes(directive)).toBe(true);
  expect(csp.includes("'unsafe-eval'")).toBe(false);
  expect((await shell.text()).includes(session)).toBe(false);
  expect((await page.request.get(`${dashboard.url}/api/v1/sessions`)).status()).toBe(401);
  const missing = await page.request.get(`${dashboard.url}/api/v1/not-a-dashboard-page`);
  expect(missing.status()).toBe(404);
  expect((await missing.text()).includes('<html')).toBe(false);
  const protectedResponse = await dashboard.request('/sessions');
  expect(protectedResponse.headers.get('cache-control')).toContain('no-store');
  expect((await dashboard.request('/sessions', 'GET', undefined, { origin: 'https://unrelated.invalid' })).status).toBe(403);
  const urls: string[] = [], consoleSecrets: boolean[] = [], external: string[] = [];
  page.on('request', (request) => { urls.push(request.url()); if (request.url().startsWith('https://dashboard-beacon.invalid/')) external.push(request.url()); });
  page.on('console', (message) => consoleSecrets.push(message.text().includes(dashboard.password)));
  await page.addInitScript(() => {
    const policy: { credentials?: RequestCredentials; redirect?: RequestRedirect; explicit: boolean }[] = [];
    Object.defineProperty(window, '__fetchPolicy', { value: policy });
    const fetchOriginal = window.fetch.bind(window);
    window.fetch = (input, options) => {
      if (String(input).includes('/api/')) policy.push({ credentials: options?.credentials, redirect: options?.redirect, explicit: new Headers(options?.headers).has('authorization') });
      return fetchOriginal(input, options);
    };
  });
  await login(page, dashboard);
  await select(page, 'Security acceptance');
  await send(page, 'HOSTILE_BROWSER');
  await expect(page.getByText('Hostile transcript fixture', { exact: false })).toBeVisible();
  expect(await page.evaluate(() => Reflect.get(window, '__dashboardPwned') === true)).toBe(false);
  expect(await page.locator('iframe, img[src^="https://dashboard-beacon.invalid"]').count()).toBe(0);
  expect(external).toHaveLength(0);
  const storage = await page.evaluate(async () => ({ local: localStorage.length, session: sessionStorage.length, databases: (await indexedDB.databases()).length, workers: (await navigator.serviceWorker.getRegistrations()).length }));
  expect(storage).toEqual({ local: 0, session: 0, databases: 0, workers: 0 });
  expect(await context.cookies()).toEqual([]);
  expect(urls.some((url) => url.includes('auth_token=') || url.includes(dashboard.password))).toBe(false);
  expect(consoleSecrets.some(Boolean)).toBe(false);
  const policy = await page.evaluate(() => Reflect.get(window, '__fetchPolicy') as { credentials?: string; redirect?: string; explicit: boolean }[]);
  expect(policy.length).toBeGreaterThan(0);
  expect(policy.every((entry) => entry.credentials === 'omit' && entry.redirect === 'error')).toBe(true);
  expect(policy.some((entry) => entry.explicit)).toBe(true);
  // A direct fetch without an explicit header must not recover ambient Basic auth.
  expect(await page.evaluate(async () => (await fetch('/api/v1/sessions', { credentials: 'omit', redirect: 'error' })).status)).toBe(401);
  await page.reload();
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
  expect((await page.request.get(`${dashboard.url}/api/v1/sessions`)).status()).toBe(401);
  expect(await dashboard.file('beacon')).toBeUndefined();
  await eventually(() => dashboard.api<{ status: string }>(`/sessions/${session}`), (state) => state.status === 'idle');
});

test('logout erases usable credentials and failed login never displays the password', async ({ dashboard, page }) => {
  await login(page, dashboard);
  await page.getByRole('button', { name: 'Logout', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
  expect((await page.request.get(`${dashboard.url}/api/v1/sessions`)).status()).toBe(401);
  await page.getByLabel('Password', { exact: true }).fill('deliberately-wrong-fixture-password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  expect((await page.locator('body').innerText()).includes('deliberately-wrong-fixture-password')).toBe(false);
});
