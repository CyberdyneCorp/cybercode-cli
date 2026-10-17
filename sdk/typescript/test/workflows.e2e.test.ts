// `client-sdk` → Workflow helpers against the real `cyber` binary: a private embedded server
// (`cyber serve --stdio`) and the shared HTTP server, both answered by a local scripted
// model. Set `CYBER_BIN` to the binary (CI builds `target/debug/cyber`); without it these
// tests are skipped.
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test, type TestContext } from "node:test";
import { Cyber, readPassword, readRegistration, type EventEnvelope, type WorkflowRunHandle } from "../src/index.js";
import { startScriptedProvider, type ScriptedProvider } from "./scripted-provider.js";

const binary = process.env.CYBER_BIN;
const skip = binary ? false : "set CYBER_BIN to the cyber binary to run the end-to-end tests";

/** An inline Workflow whose one agent waits at the model until gate `label` opens. */
function gated(label: string): string {
  return `export const meta = { name: 'g${label}', description: 'Gated', phases: [{ title: 'Wait' }] };
export default async () => { phase('Wait'); return (await agent('GATE-${label}', { label: '${label}' })).text; };
`;
}

/**
 * A fresh `CYBER_HOME` with the scripted model as the global default and at most two
 * running runs, made this process's environment so the servers it spawns use it.
 */
async function isolate(t: TestContext): Promise<{ repo: string; model: ScriptedProvider }> {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "cyber-sdk-e2e-")));
  const model = await startScriptedProvider();
  const config = join(root, "cyber-home", "config");
  mkdirSync(config, { recursive: true });
  writeFileSync(join(config, "cyber.jsonc"), JSON.stringify({ ...model.config(), workflows: { max_running_runs: 2 } }));
  writeFileSync(join(root, "models.json"), "{}");
  const repo = join(root, "repo");
  mkdirSync(join(repo, ".git"), { recursive: true });
  for (const name of Object.keys(process.env)) {
    if (name.startsWith("CYBER_") || name.startsWith("XDG_")) delete process.env[name];
  }
  Object.assign(process.env, { HOME: join(root, "home"), CYBER_HOME: join(root, "cyber-home"), CYBER_MODELS_PATH: join(root, "models.json") });
  t.after(async () => {
    await model.close();
    rmSync(root, { recursive: true, force: true });
  });
  return { repo, model };
}

async function eventually(what: string, check: () => boolean | Promise<boolean>): Promise<void> {
  const deadline = Date.now() + 30_000;
  while (!(await check())) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

async function collect<T>(items: AsyncIterable<T>): Promise<T[]> {
  const all: T[] = [];
  for await (const item of items) all.push(item);
  return all;
}

test("Attach to running runs: list() returns a handle per active run across pages", { skip, timeout: 120_000 }, async (t) => {
  const { repo, model } = await isolate(t);
  const server = await Cyber.start({ mode: "embedded", binary, directory: repo });
  t.after(() => server.close());
  const { client } = server;

  const started: WorkflowRunHandle[] = [];
  for (const label of ["a", "b", "c"]) started.push(await client.workflows.run({ script: gated(label), max_concurrent: 1 }));
  await eventually("a and b at the model", () => model.held().join() === "a,b");
  await eventually("c queued", async () => (await started[2]!.status()).status === "queued");

  // One run per page, so the three handles come from three pages read as they are reached.
  const handles = await collect(client.workflows.list({ status: ["running", "queued"], limit: 1 }));
  assert.equal(handles.length, 3);
  const byId = new Map(handles.map((h) => [h.id, h]));
  const [a, b, c] = started.map((s) => byId.get(s.id)!);
  for (const running of [a!, b!]) {
    assert.equal(running.summary?.status, "running");
    assert.equal(running.summary?.share, 1);
    assert.deepEqual(running.summary?.agents, { total: 1, completed: 0, running: 1, queued: 0, failed: 0, cancelled: 0, interrupted: 0 });
  }
  assert.equal(c!.summary?.status, "queued");
  assert.equal(c!.summary?.queue_position, 1);
  assert.deepEqual(await collect(client.workflows.list({ status: "completed" })), []);

  const results = Promise.all(handles.map((h) => h.result({ pollMs: 50 })));
  for (const label of ["a", "b", "c"]) model.open(label);
  const byRun = Object.fromEntries((await results).map((value, i) => [handles[i]!.id, value]));
  assert.deepEqual([a, b, c].map((h) => byRun[h!.id]), ["a done", "b done", "c done"]);
  const done = await collect(client.workflows.list({ status: "completed" }));
  assert.equal(done.length, 3);
  assert.ok(done.every((h) => h.summary?.agents.completed === 1));
});

test("get() attaches to a run started over HTTP and follows its events and result", { skip, timeout: 120_000 }, async (t) => {
  const { repo, model } = await isolate(t);
  const handle = await Cyber.start({ mode: "spawn", binary, directory: repo });
  t.after(() => handle.close());

  // Another client starts the run with plain HTTP requests.
  const registration = (await readRegistration())!;
  const headers = {
    authorization: `Basic ${btoa(`cyber:${await readPassword()}`)}`,
    "content-type": "application/json",
    "x-cyber-directory": repo,
  };
  const post = async (path: string, body: unknown) => {
    const res = await fetch(new URL(path, registration.url), { method: "POST", headers, body: JSON.stringify(body) });
    assert.equal(res.status, 201, await res.clone().text());
    return ((await res.json()) as { data: { id: string } }).data;
  };
  const session = await post("/api/v1/sessions", { title: "owner" });
  const started = await post("/api/v1/workflows/runs", { session_id: session.id, script: gated("h") });
  await eventually("h at the model", () => model.held().join() === "h");

  const run = await handle.client.workflows.get(started.id);
  assert.equal(run.id, started.id);
  assert.equal(run.summary?.status, "running");
  assert.equal(run.summary?.session_id, session.id);
  assert.equal(run.summary?.agents.running, 1);

  const seen: EventEnvelope[] = [];
  const controller = new AbortController();
  const following = (async () => {
    for await (const event of run.events({ signal: controller.signal })) {
      seen.push(event);
      if (event.type.startsWith("workflow.run.completed")) break;
    }
  })();
  // The stream is open once an event of the run has arrived; a release before then could be missed.
  await new Promise((resolve) => setTimeout(resolve, 1_000));
  model.open("h");
  assert.equal(await run.result({ pollMs: 50 }), "h done");
  await Promise.race([following, new Promise((resolve) => setTimeout(resolve, 10_000))]);
  controller.abort();
  const types = seen.map((e) => e.type);
  assert.ok(types.some((type) => type.startsWith("workflow.run.completed")), types.join());
  assert.ok(types.includes("workflow.run.progress"), types.join());
  assert.ok(types.every((type) => type.startsWith("workflow.")), types.join());
  for (const event of seen.filter((e) => e.type === "workflow.run.progress")) assert.equal((event.data as { run_id: string }).run_id, started.id);
});
