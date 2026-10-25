/** Production binary, real HTTP/SSE model, temporary Git checkout and private state.
 * No account/provider credential or native-vault operation is used by these fixtures.
 */
import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { createServer as createHttpServer, type Server as HttpServer } from 'node:http';
import { createServer as createTcpServer, connect, type Server as TcpServer, type Socket } from 'node:net';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join, delimiter } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';

export type Json = Record<string, unknown>;
export interface Pending { id: string; session_id: string; kind: string; metadata?: Json; resources?: string[] }
const repository = resolve(fileURLToPath(new URL('../../', import.meta.url)));
export const binary = process.env.CYBER_E2E_BINARY ?? join(repository, 'target/debug', process.platform === 'win32' ? 'cyber.exe' : 'cyber');
export const hostile = 'Hostile transcript fixture\n\n<script>globalThis.__dashboardPwned = true</script>\n<img src="https://dashboard-beacon.invalid/pixel" onerror="globalThis.__dashboardPwned=true">\n![remote](https://dashboard-beacon.invalid/markdown)\n[unsafe](javascript:alert(1))\n<iframe src="https://dashboard-beacon.invalid/frame"></iframe>';

export async function eventually<T>(read: () => Promise<T>, done: (value: T) => boolean, timeout = 15_000): Promise<T> {
  const deadline = Date.now() + timeout;
  while (true) {
    const value = await read();
    if (done(value)) return value;
    if (Date.now() >= deadline) throw new Error('Timed out waiting for fixture state');
    await new Promise((accept) => setTimeout(accept, 50));
  }
}
async function listen(server: HttpServer | TcpServer): Promise<number> {
  await new Promise<void>((accept, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', accept); });
  return (server.address() as { port: number }).port;
}
async function unusedPort(): Promise<number> {
  const server = createTcpServer();
  const port = await listen(server);
  await new Promise<void>((accept) => server.close(() => accept()));
  return port;
}
function plain(value: unknown): string {
  if (typeof value === 'string') return value;
  if (Array.isArray(value)) return value.map((part) => plain(part)).join('\n');
  if (value && typeof value === 'object' && 'text' in value) return String(value.text);
  return '';
}
function answer(body: Json): Json {
  const messages = Array.isArray(body.messages) ? body.messages as Json[] : [];
  const last = messages.at(-1) ?? {};
  if (last.role === 'tool') return { content: 'Completed scripted action.' };
  const text = plain(last.content);
  if (text.includes('HOSTILE_BROWSER')) return { content: hostile };
  let name: string;
  let input: Json;
  if(text.includes('WORKFLOW_BROWSER')) {
    name='workflow';input={script:"export const meta={name:'browser-waiting',description:'Browser lifecycle fixture'}; export default async()=>ask({question:'Wait for browser workflow review?',options:['Continue','Stop']});"};
  } else if (text.includes('QUESTION_BROWSER')) {
    name = 'question';
    input = { questions: [
      { header: 'Environment', question: 'Which environment should we use?', options: [{ label: 'Development' }, { label: 'Staging' }], allow_custom: true },
      { header: 'Checks', question: 'Which checks should run?', options: [{ label: 'Unit tests' }, { label: 'Integration tests' }], multi_select: true, allow_custom: false },
    ] };
  } else {
    const marker = /WRITE_BROWSER:([a-zA-Z0-9_-]+)/u.exec(text)?.[1];
    if (marker) { name = 'write'; input = { path: `${marker}.txt`, content: `approved-${marker}\n` }; }
    else if (text.includes('LONG_COMMAND_BROWSER')) { name = 'bash'; input = { command: `printf '%s' '${'readable-long-command-'.repeat(100)}' > long-command.txt` }; }
    else return { content: 'Scripted browser response.' };
  }
  return { tool_calls: [{ index: 0, id: `browser_${messages.length}`, type: 'function', function: { name, arguments: JSON.stringify(input) } }] };
}

export class DashboardFixture {
  root = '';
  home = '';
  repo = '';
  url = '';
  port = 0;
  readonly password = `browser-fixture-${randomUUID()}`;
  readonly modelRequests: Json[] = [];
  private model: HttpServer | undefined;
  private server: ChildProcess | undefined;
  private processOutput = '';
  private env: NodeJS.ProcessEnv = {};
  private closed = false;

  static async create(extraConfig: Json = {}): Promise<DashboardFixture> {
    const fixture = new DashboardFixture();
    try { await fixture.prepare(extraConfig); return fixture; }
    catch (error) { await fixture.close(); throw error; }
  }
  get authorization(): string { return `Basic ${Buffer.from(`cyber:${this.password}`).toString('base64')}`; }
  private async prepare(extraConfig: Json): Promise<void> {
    this.root = await mkdtemp(join(tmpdir(), 'cyber-browser-'));
    this.home = join(this.root, 'installation');
    this.repo = join(this.root, 'checkout');
    await Promise.all([mkdir(this.repo), mkdir(join(this.root, 'home')), mkdir(join(this.home, 'config'), { recursive: true }), mkdir(join(this.home, 'state'), { recursive: true })]);
    this.model = createHttpServer(async (request, response) => {
      const chunks: Buffer[] = [];
      for await (const part of request) chunks.push(Buffer.from(part));
      const body = JSON.parse(Buffer.concat(chunks).toString()) as Json;
      this.modelRequests.push(body);
      const delta = answer(body);
      const chunk = { choices: [{ index: 0, delta, finish_reason: 'tool_calls' in delta ? 'tool_calls' : 'stop' }], usage: { prompt_tokens: 100, completion_tokens: 10 } };
      response.writeHead(200, { 'content-type': 'text/event-stream' });
      response.end(`data: ${JSON.stringify(chunk)}\n\ndata: [DONE]\n\n`);
    });
    const modelPort = await listen(this.model);
    await writeFile(join(this.root, 'models.json'), '{}');
    await writeFile(join(this.home, 'state/password'), this.password, { mode: 0o600 });
    await writeFile(join(this.home, 'config/cyber.json'), JSON.stringify({
      providers: { test: { api: { type: 'openai-compatible', url: `http://127.0.0.1:${modelPort}/v1`, settings: { auth: 'none' } }, models: { main: {} } } },
      model: 'test/main', permissions: { edit: 'ask', bash: 'ask' }, ...extraConfig,
    }));
    this.env = { PATH: process.env.PATH, HOME: join(this.root, 'home'), CYBER_HOME: this.home, CYBER_MODELS_PATH: join(this.root, 'models.json'), CYBER_OFFLINE: '1', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: join(this.root, 'empty-git-config'), ...process.platform === 'win32' ? { SystemRoot: process.env.SystemRoot } : {} };
    await writeFile(join(this.root, 'empty-git-config'), '');
    for (const args of [['init', '--quiet', '-b', 'main'], ['config', 'user.name', 'Browser fixture'], ['config', 'user.email', 'fixture@example.invalid'], ['-c', 'user.name=Browser fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--allow-empty', '--quiet', '-m', 'fixture']]) {
      const result = spawnSync('git', args, { cwd: this.repo, env: this.env, encoding: 'utf8' });
      if (result.status !== 0) throw new Error('Could not initialize isolated fixture checkout');
    }
    this.port = await unusedPort();
    this.url = `http://127.0.0.1:${this.port}`;
  }
  async configureTls(): Promise<void> {
    const certificate=join(this.root,'server-cert.pem'), key=join(this.root,'server-key.pem');
    const result=spawnSync('openssl',['req','-x509','-newkey','rsa:2048','-nodes','-keyout',key,'-out',certificate,'-days','1','-subj','/CN=localhost','-addext','subjectAltName=DNS:localhost,IP:127.0.0.1'],{cwd:this.root,encoding:'utf8'});
    if(result.status!==0)throw new Error('Could not generate isolated TLS fixture certificate');
    const path=join(this.home,'config/cyber.json');const config=JSON.parse(await readFile(path,'utf8')) as Json;config.server={tls:{cert:certificate,key}};await writeFile(path,JSON.stringify(config));this.url=`https://127.0.0.1:${this.port}`;
  }
  async start(options: { hostname?: string; probe?: () => Promise<boolean> } = {}): Promise<void> {
    if (this.server) throw new Error('Fixture server already started');
    this.server = spawn(binary, ['serve', '--register', '--hostname', options.hostname ?? '127.0.0.1', '--port', String(this.port), '--sandbox', 'full-access'], { cwd: this.repo, env: this.env, stdio: ['ignore', 'pipe', 'pipe'] });
    for (const stream of [this.server.stdout, this.server.stderr]) stream?.on('data', (chunk: Buffer) => { this.processOutput += chunk.toString(); });
    await eventually(async () => {
      if (this.server?.exitCode !== null) throw new Error(`Fixture server exited: ${this.logs()}`);
      if(options.probe)return options.probe().catch(()=>false);
      return fetch(`${this.url}/api/v1/health`, { signal: AbortSignal.timeout(500) }).then((response) => response.ok).catch(() => false);
    }, Boolean, 30_000);
  }
  async stop(): Promise<void> {
    const child = this.server;
    if (!child) return;
    child.kill('SIGTERM');
    await Promise.race([new Promise<void>((accept) => child.once('exit', () => accept())), new Promise<void>((accept) => setTimeout(accept, 5_000))]);
    if (child.exitCode === null) child.kill('SIGKILL');
    this.server = undefined;
  }
  async restartWithTrustedPath(directory: string): Promise<void> {
    await this.stop();this.env.PATH=`${directory}${delimiter}${this.env.PATH ?? ''}`;await this.start();
  }
  async crash(): Promise<void> {
    const child=this.server;if(!child)return;
    const ended=new Promise<void>(accept=>child.once('exit',()=>accept()));
    child.kill('SIGKILL');await ended;this.server=undefined;
  }
  async restart(): Promise<void> { await this.stop(); await this.start(); }
  async terminal(args: string[]): Promise<{ code: number; stdout: string; stderr: string }> {
    const child = spawn(binary, args, { cwd: this.repo, env: this.env, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '', stderr = '';
    child.stdout.on('data', (data: Buffer) => { stdout += data.toString(); });
    child.stderr.on('data', (data: Buffer) => { stderr += data.toString(); });
    const timeout = setTimeout(() => child.kill('SIGKILL'), 20_000);
    try { const code = await new Promise<number>((accept, reject) => { child.once('error', reject); child.once('exit', (status) => accept(status ?? -1)); }); return { code, stdout, stderr }; }
    finally { clearTimeout(timeout); }
  }
  async request(path: string, method = 'GET', body?: unknown, extraHeaders: Record<string, string> = {}): Promise<Response> {
    return fetch(`${this.url}/api/v1${path}`, { method, headers: { authorization: this.authorization, 'x-cyber-directory': this.repo, ...(body === undefined ? {} : { 'content-type': 'application/json' }), ...extraHeaders }, ...(body === undefined ? {} : { body: JSON.stringify(body) }), redirect: 'error', signal: AbortSignal.timeout(10_000) });
  }
  async api<T>(path: string, method = 'GET', body?: unknown): Promise<T> {
    const response = await this.request(path, method, body);
    if (!response.ok) throw new Error(`${method} ${path} returned ${response.status}`);
    if (response.status === 204) return undefined as T;
    const envelope = await response.json() as { data?: T };
    return (envelope.data ?? envelope) as T;
  }
  async session(title: string): Promise<string> {
    const session = await this.api<{ id: string }>('/sessions', 'POST', { title, directory: this.repo, model: 'test/main', mode: 'default' });
    return session.id;
  }
  async prompt(session: string, text: string): Promise<void> { await this.api(`/sessions/${session}/prompt`, 'POST', { parts: [{ type: 'text', text }], delivery: 'steer' }); }
  async pending(kind: 'permissions' | 'questions', session: string): Promise<Pending[]> { return this.api(`/${kind}/requests?session_id=${session}`); }
  async waitPending(kind: 'permissions' | 'questions', session: string): Promise<Pending> { return (await eventually(() => this.pending(kind, session), (rows) => rows.length > 0))[0]!; }
  async file(name: string): Promise<string | undefined> { return readFile(join(this.repo, name), 'utf8').catch(() => undefined); }
  logs(): string { return this.processOutput.replaceAll(this.password, '[redacted]').replaceAll(this.authorization, '[redacted]'); }
  async forward(): Promise<{ url: string; close: () => Promise<void> }> {
    const sockets = new Set<Socket>();
    const forwarding = createTcpServer((incoming) => {
      const outgoing = connect(this.port, '127.0.0.1');
      for (const socket of [incoming, outgoing]) { sockets.add(socket); socket.once('close', () => sockets.delete(socket)); socket.on('error', () => socket.destroy()); }
      incoming.pipe(outgoing).pipe(incoming);
    });
    const port = await listen(forwarding);
    return { url: `http://127.0.0.1:${port}`, close: async () => { for (const socket of sockets) socket.destroy(); await new Promise<void>((accept) => forwarding.close(() => accept())); } };
  }
  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    await this.stop();
    if (this.model) await new Promise<void>((accept) => { this.model!.closeAllConnections(); this.model!.close(() => accept()); });
    if (this.root) await rm(this.root, { recursive: true, force: true });
  }
}
