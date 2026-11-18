/**
 * The extension against the real `cyber` server (`editor-integration` scenarios). Tests run
 * in order and share the panel's Session.
 */
import { strict as assert } from "node:assert";
import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import * as vscode from "vscode";
import type { TestApi } from "../../../src/extension/extension";
import type { FixtureInfo } from "../fixture";
import { CHANGED } from "../../../src/extension/review";
import { SCOPE_MESSAGE } from "../../../src/extension/api";

const fixture = JSON.parse(process.env.CYBER_TEST_FIXTURE ?? "{}") as FixtureInfo;
const appPath = join(fixture.repo ?? "", "src/app.ts");

export async function eventually<T>(read: () => T | Promise<T>, done: (value: T) => boolean, what: string, timeout = 20_000): Promise<T> {
  const deadline = Date.now() + timeout;
  for (;;) {
    const value = await read();
    if (done(value)) return value;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

/** The first defined value `read` returns. */
export async function present<T>(read: () => T | undefined | Promise<T | undefined>, what: string): Promise<T> {
  return (await eventually(read, (value) => value !== undefined && value !== null && value !== "", what)) as T;
}

export async function extensionApi(): Promise<TestApi> {
  const extension = vscode.extensions.getExtension<TestApi>("cyber-code.cyber");
  assert.ok(extension, "the extension is installed");
  const api = await extension.activate();
  assert.ok(api, "test hooks are exported under CYBER_VSCODE_TEST=1");
  return api;
}

function token(): string {
  return readFileSync(join(fixture.home, "state/ide/vscode.token"), "utf8").split("\n")[0] ?? "";
}

function cli(args: string[]): string {
  const result = spawnSync(fixture.binary, args, { cwd: fixture.repo, env: process.env, encoding: "utf8", timeout: 30_000 });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout;
}

async function server(path: string, method = "GET", body?: unknown): Promise<Response> {
  return fetch(`${fixture.serverUrl}/api/v1${path}`, {
    method,
    headers: { authorization: `Bearer ${token()}`, "x-cyber-directory": fixture.repo, ...(body === undefined ? {} : { "content-type": "application/json" }) },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
}

async function modelRequests(): Promise<string> {
  return JSON.stringify(await (await fetch(fixture.modelUrl)).json());
}

function registrationId(): string {
  return JSON.parse(readFileSync(join(fixture.home, "state/server.json"), "utf8")).id;
}

function diffTabs(): vscode.Tab[] {
  return vscode.window.tabGroups.all.flatMap((group) => group.tabs).filter((tab) => {
    const input = tab.input;
    if (input instanceof vscode.TabInputTextDiff) return input.modified.scheme === "cyber-review";
    return input instanceof vscode.TabInputText && input.uri.scheme === "cyber-review";
  });
}

/** Open `path` in an editor. */
async function openEditor(path: string): Promise<vscode.TextEditor> {
  const document = await vscode.workspace.openTextDocument(vscode.Uri.file(path));
  return vscode.window.showTextDocument(document, { viewColumn: vscode.ViewColumn.One, preview: false });
}

/** What the server and the model saw, for a failure message. */
async function diagnose(api: TestApi): Promise<string> {
  const session = api.chat.state.sessionId;
  const pending = await (await server("/permissions/requests")).text();
  const status = await (await server(`/sessions/${session}`)).text();
  const requests = JSON.parse(await modelRequests()) as { messages?: { role: string; content?: unknown; tool_calls?: { function: { name: string } }[] }[] }[];
  const last = requests.at(-1)?.messages?.slice(-4).map((m) => `${m.role}:${JSON.stringify(m.tool_calls?.map((c) => c.function.name) ?? m.content).slice(0, 300)}`);
  return `events=${api.chat.eventsSeen} bound=${api.chat.table.session} log=${JSON.stringify(api.logged.slice(-12))} cards=${JSON.stringify(api.chat.state.cards)} notice=${api.chat.state.notice} pending=${pending.slice(0, 500)} session=${status.slice(0, 400)} model=${JSON.stringify(last)}`;
}

async function card(api: TestApi, kind: "permission" | "question") {
  try {
    return await present(() => api.chat.state.cards.find((c) => c.kind === kind && c.state === "pending"), `a pending ${kind} card`);
  } catch (error) {
    throw new Error(`${String(error)}: ${await diagnose(api)}`);
  }
}

async function prompt(api: TestApi, text: string, includeContext = false): Promise<void> {
  await api.chat.receive({ type: "prompt", text, includeContext });
}

async function idle(api: TestApi): Promise<void> {
  await eventually(async () => (await (await server(`/sessions/${api.chat.state.sessionId}`)).json()) as { data: { status: string } }, (s) => s.data.status === "idle", "an idle Session");
}

export function chatSuite(): void {
  let api: TestApi;
  let registered: string;

  suiteSetup(async () => {
    registered = registrationId();
    api = await extensionApi();
    await vscode.commands.executeCommand("cyber.chat.focus");
    await eventually(() => api.chat.state.phase, (phase) => phase === "ready", "the panel to attach");
  });

  test("Panel attaches to the service without starting another", async () => {
    assert.equal(registrationId(), registered);
    assert.equal(api.chat.state.notice, "");
    assert.ok(api.stateReads.some((path) => path.endsWith("server.json")));
  });

  test("Shared with the TUI: a prompt from the panel streams the reply into the same Session", async () => {
    await prompt(api, "hello from the panel");
    const id = await eventually(() => api.chat.state.sessionId, Boolean, "a bound Session");
    await eventually(() => api.chat.state.messages.map((m) => m.text).join("\n"), (text) => text.includes("Scripted reply from the fixture model."), "the streamed reply");
    const messages = await (await server(`/sessions/${id}/messages`)).json();
    assert.ok(JSON.stringify(messages).includes("hello from the panel"), "another client sees the same Session");
  });

  test("Selection sent as a ranged file part", async () => {
    const editor = await openEditor(appPath);
    editor.selection = new vscode.Selection(1, 0, 3, 5);
    await eventually(() => api.chat.state.chip?.label, (label) => label === "src/app.ts#L2-4", "the context chip", 5000).catch(() => undefined);
    await prompt(api, "explain this", true);
    await eventually(modelRequests, (text) => text.includes('<file path=\\"src/app.ts\\" lines=\\"2-4\\">'), "a ranged file block");
    await idle(api);
  });

  test("Denied file withheld", async () => {
    await openEditor(join(fixture.repo, ".env"));
    await prompt(api, "what is in here", true);
    await eventually(() => api.chat.state.notice, (notice) => /denied|deny/i.test(notice), "the denial");
    assert.ok(!(await modelRequests()).includes("do-not-admit"));
  });

  test("Forged approval refused", async () => {
    const before = api.logged.length;
    await api.chat.receive({ type: "reply", sessionId: api.chat.state.sessionId, requestId: "per_123", reply: "once" });
    assert.ok(api.logged.slice(before).some((line) => line.includes("dropped a reply")));
    assert.ok(!api.logged.slice(before).some((line) => line.includes("/reply")));
  });

  test("Accept a proposed edit", async () => {
    await prompt(api, "EDIT_APP please");
    const pending = await card(api, "permission");
    assert.equal(pending.hasDiff, true);
    await api.chat.receive({ type: "review", requestId: pending.requestId });
    await eventually(diffTabs, (tabs) => tabs.some((tab) => tab.input instanceof vscode.TabInputTextDiff), "the diff editor");
    assert.equal(api.reviews.active, pending.requestId);
    await vscode.commands.executeCommand("cyber.review.accept");
    await eventually(() => readFileSync(appPath, "utf8"), (text) => text.includes("const value = 2;"), "the server's write");
    await eventually(diffTabs, (tabs) => tabs.length === 0, "the review to close");
    await idle(api);
  });

  test("File changed before review, then Reject with feedback", async () => {
    await prompt(api, "EDIT_APP again");
    const pending = await card(api, "permission");
    writeFileSync(appPath, readFileSync(appPath, "utf8").replace("const value = 2;", "const value = 3;"));
    await api.chat.receive({ type: "review", requestId: pending.requestId });
    await eventually(diffTabs, (tabs) => tabs.some((tab) => tab.label.includes("proposal.diff")), "the raw diff");
    const raw = vscode.workspace.textDocuments.find((doc) => doc.uri.scheme === "cyber-review" && doc.uri.path.includes("-raw/"));
    assert.ok(raw?.getText().includes(CHANGED));
    assert.ok(await api.chat.reply(pending.sessionId, pending.requestId, { reply: "reject", message: "use the existing helper" }));
    await eventually(modelRequests, (text) => text.includes("use the existing helper"), "the feedback reaching the model");
    assert.ok(readFileSync(appPath, "utf8").includes("const value = 3;"));
    await idle(api);
  });

  test("Answered in another client", async () => {
    await prompt(api, "WRITE_NEW:elsewhere");
    const pending = await card(api, "permission");
    await api.chat.receive({ type: "review", requestId: pending.requestId });
    await eventually(diffTabs, (tabs) => tabs.length > 0, "the review");
    const response = await server(`/sessions/${pending.sessionId}/permissions/${pending.requestId}/reply`, "POST", { reply: "once" });
    assert.equal(response.status, 204);
    await eventually(diffTabs, (tabs) => tabs.length === 0, "the review to close");
    assert.equal(await api.chat.reply(pending.sessionId, pending.requestId, { reply: "reject" }), false, "a stale card sends nothing");
    await idle(api);
  });

  test("Plan approval as a question", async () => {
    await api.chat.receive({ type: "setMode", mode: "plan" });
    await eventually(() => api.chat.state.mode, (mode) => mode === "plan", "plan mode").catch(async (error: unknown) => {
      throw new Error(`${String(error)}: ${await diagnose(api)}`);
    });
    await prompt(api, "PLAN_EXIT now");
    const question = await card(api, "question");
    assert.deepEqual(question.questions[0]?.options.slice(0, 2), ["Approve and build", "Approve with accept-edits"]);
    assert.ok(question.questions[0]?.options.includes("Request changes"));
    assert.ok(await api.chat.reply(question.sessionId, question.requestId, { answers: [["Approve and build"]] }));
    await eventually(() => api.chat.state.mode, (mode) => mode === "default", "default mode after approval");
    await idle(api);
  });

  test("Out-of-scope action", async () => {
    const client = api.chat.client;
    assert.ok(client);
    const error = await client.request("v1.agent.list").then(() => undefined, (e: unknown) => e);
    await api.chat.failed(error);
    assert.equal(api.chat.state.notice, SCOPE_MESSAGE);
  });

  test("Continue in the TUI", async () => {
    await vscode.commands.executeCommand("cyber.openSessionInTerminal");
    const terminal = await present(() => vscode.window.terminals.find((t) => t.name === "Cyber Code"), "the terminal");
    const options = terminal.creationOptions as vscode.TerminalOptions;
    assert.equal(options.shellPath, fixture.binary);
    assert.deepEqual(options.shellArgs, ["--resume", api.chat.state.sessionId]);
    terminal.dispose();
  });

  test("Panel hidden: the lease is released", async () => {
    await eventually(() => api.chat.leaseId, Boolean, "a presence lease");
    await vscode.commands.executeCommand("workbench.view.explorer");
    await eventually(() => api.chat.leaseId, (id) => id === undefined, "the lease release");
    await vscode.commands.executeCommand("cyber.chat.focus");
    await eventually(() => api.chat.leaseId, Boolean, "a new lease");
  });

  test("Token revoked, then First run without a token", async () => {
    const id = /itk_[A-Za-z0-9]+/.exec(cli(["ide", "token", "list"]).split("\n").find((line) => line.includes("active")) ?? "")?.[0];
    assert.ok(id);
    cli(["ide", "token", "revoke", id]);
    await api.chat.refreshPending().catch((error: unknown) => api.chat.failed(error));
    await eventually(() => api.chat.state.phase, (phase) => phase === "noToken", "Create IDE token to be offered");
    assert.deepEqual(api.chat.state.actions, ["createToken"]);
    cli(["ide", "token", "create", "--client", "vscode"]);
    await eventually(() => api.chat.state.phase, (phase) => phase === "ready", "the panel to attach with the new token");
  });

  test("No persisted credential reaches the webview, the log or the password file", async () => {
    const secret = token();
    const posted = JSON.stringify(api.posted);
    assert.ok(!posted.includes(secret) && !posted.includes("cyi_"));
    assert.ok(!api.logged.join("\n").includes("cyi_"));
    assert.ok(!api.stateReads.some((path) => path.endsWith("password")), api.stateReads.join(", "));
    const settings = JSON.stringify(vscode.workspace.getConfiguration("cyber"));
    assert.ok(!settings.includes("cyi_"));
  });
}
