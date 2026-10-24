import { test, expect, login, select, send, tab } from './browser.js';
import { eventually } from './fixture.js';

test('Runs binds real workflow pause/resume/stop, Job output/stop and Goal controls without automatic candidate application', async ({ dashboard, page }) => {
  const session = await dashboard.session('Work monitor acceptance');
  await login(page, dashboard); await select(page, 'Work monitor acceptance');
  await send(page,'WORKFLOW_BROWSER');
  const permission=await dashboard.waitPending('permissions',session);
  await tab(page,'Approvals');
  await page.getByRole('article',{name:`Permission request ${permission.id}`}).getByRole('button',{name:'Approve once',exact:true}).click();
  const runs=await eventually(()=>dashboard.api<{id:string;status:string}[]>(`/workflows/runs?session_id=${session}`),rows=>rows.some(run=>run.status==='waiting_input'));
  const run=runs.find(run=>run.status==='waiting_input')!;
  await tab(page, 'Runs');
  await expect(page.getByRole('button', { name: 'Pause workflow', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Pause workflow', exact: true }).click();
  await eventually(() => dashboard.api<{ status: string }>(`/workflows/runs/${run.id}`), (state) => state.status === 'paused');
  await page.getByRole('button', { name: 'Resume workflow', exact: true }).click();
  await eventually(() => dashboard.api<{ status: string }>(`/workflows/runs/${run.id}`), (state) => state.status === 'waiting_input');
  await page.getByRole('button', { name: 'View run evidence', exact: true }).click();
  await expect(page.getByText('No server-owned solve evidence attached.', { exact: false })).toBeVisible();
  expect(await page.getByRole('button', { name: /apply candidate|merge candidate|publish candidate/iu }).count()).toBe(0);
  await page.getByRole('button', { name: 'Stop workflow', exact: true }).click();
  await eventually(() => dashboard.api<{ status: string }>(`/workflows/runs/${run.id}`), (state) => !['running', 'waiting_input', 'queued', 'paused'].includes(state.status));

  const shell = await dashboard.api<{ job: { id: string } }>(`/sessions/${session}/shell`, 'POST', { command: "printf 'browser-job-output\\n'; sleep 30", background: true });
  await expect(page.getByRole('button', { name: 'View Job output', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'View Job output', exact: true }).click();
  await expect(page.getByLabel('Job output', { exact: true })).toContainText('browser-job-output');
  await page.getByRole('button', { name: 'Stop Job', exact: true }).click();
  await eventually(() => dashboard.api<{ status: string }>(`/jobs/${shell.job.id}`), (state) => state.status !== 'running');

  await dashboard.api(`/sessions/${session}/goals`, 'POST', { condition: 'QUESTION_BROWSER: explicit Goal review', budget: { max_tokens: 10_000, max_wall_seconds: 60 } });
  await dashboard.api(`/sessions/${session}/goals/pause`, 'POST', {});
  await expect(page.getByRole('button', { name: 'Resume current Goal', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Resume current Goal', exact: true }).click();
  await eventually(() => dashboard.api<{ current: { status: string } }>(`/sessions/${session}/goals`), (state) => state.current?.status === 'active');
  await page.getByRole('button', { name: 'Pause current Goal', exact: true }).click();
  await eventually(() => dashboard.api<{ current: { status: string } }>(`/sessions/${session}/goals`), (state) => state.current?.status === 'paused');
  await page.getByRole('button', { name: 'Clear Goal', exact: true }).click();
  await eventually(() => dashboard.api<{ current: unknown }>(`/sessions/${session}/goals`), (state) => state.current === null);
  for (const question of await dashboard.pending('questions', session)) await dashboard.api(`/sessions/${session}/questions/${question.id}/reply`, 'POST', {});
  const headless=await dashboard.api<{id:string}>('/workflows/runs','POST',{session_id:session,script:"export const meta={name:'headless-browser-fixture',description:'Must stay headless'}; export default async()=>ask('Must not be resurrected by browser presence');"});
  await eventually(()=>dashboard.api<{status:string}>(`/workflows/runs/${headless.id}`),state=>state.status==='failed');
  await page.getByRole('button',{name:'Reconnect',exact:true}).click();
  expect((await dashboard.pending('questions',session)).length).toBe(0);

});

test('Session navigation scopes the transcript and New Session creates a real owner Session', async ({ dashboard, page }) => {
  await dashboard.session('First browser Session');
  await dashboard.session('Second browser Session');
  await login(page, dashboard);
  await select(page, 'First browser Session');
  await send(page, 'A transcript unique to the first Session');
  await expect(page.getByText('Scripted browser response.', { exact: true })).toBeVisible();
  await select(page, 'Second browser Session');
  await tab(page, 'Conversation');
  await expect(page.getByText('A transcript unique to the first Session', { exact: true })).toHaveCount(0);
  const previous = await dashboard.api<{ data: { id: string }[] }>('/sessions');
  await page.getByRole('button', { name: 'New Session', exact: true }).click();
  await eventually(() => dashboard.api<{ data: { id: string }[] }>('/sessions'), rows => rows.data.length === previous.data.length + 1);
  await expect(page.getByRole('navigation', { name: 'Sessions', exact: true }).getByRole('button', { pressed: true })).toHaveCount(1);
  await send(page, 'A fresh Session conversation');
  await expect(page.getByText('Scripted browser response.', { exact: true })).toBeVisible();
});
