import { test, expect, login, select, send, tab } from './browser.js';
import { eventually } from './fixture.js';

test('real CLI web reuse, browser prompt and approval complete a tool observed by terminal and second browser', async ({ dashboard, page, browser }) => {
  const session = await dashboard.session('Shared approval');
  const opened = await dashboard.terminal(['web', '--hostname', '127.0.0.1', '--port', String(dashboard.port), '--no-open']);
  expect(opened.code).toBe(0);
  expect(opened.stdout.includes(dashboard.url)).toBe(true);
  expect((opened.stdout + opened.stderr).includes(dashboard.password)).toBe(false);
  const secondContext = await browser.newContext();
  const second = await secondContext.newPage();
  try {
    await login(page, dashboard); await select(page, 'Shared approval');
    await login(second, dashboard); await select(second, 'Shared approval');
    await send(page, 'WRITE_BROWSER:approved');
    const request = await dashboard.waitPending('permissions', session);
    expect(await dashboard.file('approved.txt')).toBeUndefined();
    await tab(page, 'Approvals'); await tab(second, 'Approvals');
    const card = page.getByRole('article', { name: `Permission request ${request.id}` });
    await expect(card).toContainText('approved.txt');
    await expect(second.getByRole('article', { name: `Permission request ${request.id}` })).toBeVisible();
    await card.getByRole('button', { name: 'Approve once', exact: true }).click();
    await eventually(() => dashboard.file('approved.txt'), (value) => value === 'approved-approved\n');
    await expect(second.getByRole('article', { name: `Permission request ${request.id}` })).toHaveCount(0);
    const terminal = await dashboard.terminal(['api', 'GET', `/sessions/${session}/messages`]);
    expect(terminal.code).toBe(0);
    expect(terminal.stdout.includes('approved.txt')).toBe(true);
    await tab(second, 'Conversation');
    await expect(second.getByText('Completed scripted action.', { exact: false })).toBeVisible();
  } finally { await secondContext.close(); }
});

test('terminal wins a race and an old browser decision never transfers to the next request', async ({ dashboard, page }) => {
  const session = await dashboard.session('Approval race');
  await login(page, dashboard); await select(page, 'Approval race');
  await send(page, 'WRITE_BROWSER:race_one');
  const first = await dashboard.waitPending('permissions', session);
  await tab(page, 'Approvals');
  let release!: () => void;
  const gate = new Promise<void>((accept) => { release = accept; });
  const submitted: { url: string; key: string | undefined; body: string | null }[] = [];
  await page.route(`**/permissions/${first.id}/reply`, async (route) => {
    const request = route.request(); submitted.push({ url: request.url(), key: request.headers()['idempotency-key'], body: request.postData() });
    await gate; await route.continue();
  });
  await page.getByRole('article', { name: `Permission request ${first.id}` }).getByRole('button', { name: 'Approve once', exact: true }).click({ noWaitAfter: true });
  await expect.poll(() => submitted.length).toBe(1);
  await dashboard.api(`/sessions/${session}/permissions/${first.id}/reply`, 'POST', { reply: 'once' });
  await eventually(() => dashboard.file('race_one.txt'), Boolean);
  await dashboard.prompt(session, 'WRITE_BROWSER:race_two');
  const next = await dashboard.waitPending('permissions', session);
  release();
  await expect(page.getByRole('article', { name: `Permission request ${next.id}` })).toBeVisible();
  expect(submitted.every((request) => request.url.includes(`/permissions/${first.id}/reply`))).toBe(true);
  expect(await dashboard.file('race_two.txt')).toBeUndefined();
  await page.getByRole('article', { name: `Permission request ${next.id}` }).getByRole('button', { name: 'Reject and stop', exact: true }).click();
  expect(await dashboard.file('race_two.txt')).toBeUndefined();
});

test('lost approval acknowledgement preserves exact request, payload and idempotency identity', async ({ dashboard, page }) => {
  const session = await dashboard.session('Lost acknowledgement');
  await login(page, dashboard); await select(page, 'Lost acknowledgement');
  await send(page, 'WRITE_BROWSER:lost_ack');
  const pending = await dashboard.waitPending('permissions', session);
  await tab(page, 'Approvals');
  const replies: { key: string | undefined; body: string | null; url: string }[] = [];
  await page.route(`**/permissions/${pending.id}/reply`, async (route) => {
    const request = route.request(); replies.push({ key: request.headers()['idempotency-key'], body: request.postData(), url: request.url() });
    if (replies.length === 1) { await route.fetch(); await route.abort('connectionreset'); }
    else await route.continue();
  });
  await page.getByRole('article', { name: `Permission request ${pending.id}` }).getByRole('button', { name: 'Approve once', exact: true }).click();
  await eventually(() => dashboard.file('lost_ack.txt'), Boolean);
  await expect(page.getByRole('article', { name: `Permission request ${pending.id}` })).toHaveCount(0);
  expect(replies.length).toBeGreaterThan(0);
  expect(Boolean(replies[0]?.key)).toBe(true);
  expect(new Set(replies.map((reply) => reply.key)).size).toBe(1);
  expect(new Set(replies.map((reply) => reply.body)).size).toBe(1);
  expect(new Set(replies.map((reply) => reply.url)).size).toBe(1);
  expect((await dashboard.pending('permissions', session)).length).toBe(0);
});

test('an uncertain uncommitted approval retries only its exact original decision after live reconciliation',async({dashboard,page})=>{
  const session=await dashboard.session('Retry exact answer');
  await login(page,dashboard);await select(page,'Retry exact answer');
  await send(page,'WRITE_BROWSER:retry_exact');
  const pending=await dashboard.waitPending('permissions',session);
  await tab(page,'Approvals');
  const replies:{key:string|undefined;body:string|null;url:string}[]=[];
  await page.route(`**/permissions/${pending.id}/reply`,async route=>{
    const request=route.request();replies.push({key:request.headers()['idempotency-key'],body:request.postData(),url:request.url()});
    if(replies.length===1)await route.abort('connectionreset');else await route.continue();
  });
  const card=page.getByRole('article',{name:`Permission request ${pending.id}`});
  await card.getByRole('button',{name:'Approve once',exact:true}).click();
  expect(await dashboard.file('retry_exact.txt')).toBeUndefined();
  await expect(card.getByRole('button',{name:'Retry same answer',exact:true})).toBeEnabled();
  await card.getByRole('button',{name:'Retry same answer',exact:true}).click();
  await eventually(()=>dashboard.file('retry_exact.txt'),Boolean);
  expect(replies.length).toBeGreaterThanOrEqual(2);
  expect(new Set(replies.map(reply=>reply.key)).size).toBe(1);
  expect(new Set(replies.map(reply=>reply.body)).size).toBe(1);
  expect(new Set(replies.map(reply=>reply.url)).size).toBe(1);
});

test('questions support choices, multiple selections and explicit answers', async ({ dashboard, page }) => {
  const session = await dashboard.session('Question acceptance');
  await login(page, dashboard); await select(page, 'Question acceptance');
  await send(page, 'QUESTION_BROWSER');
  const pending = await dashboard.waitPending('questions', session);
  await tab(page, 'Approvals');
  const card = page.getByRole('article', { name: `Question request ${pending.id}` });
  await card.getByLabel('Development', { exact: true }).check();
  await card.getByLabel('Unit tests', { exact: true }).check();
  await card.getByLabel('Integration tests', { exact: true }).check();
  await card.getByRole('button', { name: 'Submit answers', exact: true }).click();
  await expect(card).toHaveCount(0);
  await expect.poll(async () => (await dashboard.pending('questions', session)).length).toBe(0);
  const terminal = await dashboard.terminal(['api', 'GET', `/sessions/${session}/messages`]);
  expect(terminal.code).toBe(0);
  expect(terminal.stdout.includes('Development')).toBe(true);
  expect(terminal.stdout.includes('Integration tests')).toBe(true);
  await dashboard.prompt(session,'QUESTION_BROWSER');
  const custom=await dashboard.waitPending('questions',session);
  const customCard=page.getByRole('article',{name:`Question request ${custom.id}`});
  await customCard.getByRole('textbox',{name:'Custom answer',exact:true}).fill('Preview');
  await customCard.getByLabel('Unit tests',{exact:true}).check();
  await customCard.getByRole('button',{name:'Submit answers',exact:true}).click();
  await expect(customCard).toHaveCount(0);
  await dashboard.prompt(session,'QUESTION_BROWSER');
  const dismissed=await dashboard.waitPending('questions',session);
  const dismissCard=page.getByRole('article',{name:`Question request ${dismissed.id}`});
  await dismissCard.getByRole('button',{name:'Dismiss question',exact:true}).click();
  await expect(dismissCard).toHaveCount(0);
});

test('360px and 1280px approval layouts preserve visible authority and internal command scrolling', async ({ dashboard, page }, info) => {
  await dashboard.session('Responsive approval');
  await login(page, dashboard); await select(page, 'Responsive approval');
  await send(page, 'LONG_COMMAND_BROWSER');
  await tab(page, 'Approvals');
  await expect(page.getByRole('button', { name: 'Approve once', exact: true })).toBeVisible();
  await page.screenshot({ path: info.outputPath('desktop-1280-approval.png'), fullPage: true });
  await page.setViewportSize({ width: 360, height: 800 });
  await expect(page.getByRole('button', { name: 'Approve once', exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
  await page.screenshot({ path: info.outputPath('mobile-360-approval.png'), fullPage: true });
  await page.getByRole('button', { name: 'Review saved approval', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Review saved approval' });
  await expect(dialog).toContainText('long-command');
  await expect(dialog.getByRole('button', { name: 'Approve always', exact: true })).toBeVisible();
});
