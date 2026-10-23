import assert from "node:assert/strict";
import { test } from "node:test";
import { WorkflowRunError, type RunDetail, type WorkflowRun } from "../src/index.js";
import { durable, frame, json, mockClient, sse } from "./support.js";

const run: WorkflowRun = {
  id: "run_1",
  name: "audit",
  session_id: "ses_owner",
  directory: "/repo",
  mode: "default",
  origin: { kind: "saved" },
  digest: "sha256:0",
  args: {},
  status: "running",
  phases: [],
  started_ms: 1,
};

function detail(status: RunDetail["status"], extra: Partial<RunDetail> = {}): RunDetail {
  const agents = { total: 0, completed: 0, running: 0, queued: 0, failed: 0, cancelled: 0, interrupted: 0 };
  return { ...run, status, agents, tokens: 0, cost_usd: 0, elapsed_ms: 0, agents_detail: [], ...extra };
}

test("workflows.run creates an owner Session and returns a handle over the generated operations", async () => {
  let polls = 0;
  const { client, calls } = mockClient((req) => {
    if (req.url.pathname === "/api/v1/sessions") return json(201, { location: { directory: "/repo" }, data: { id: "ses_owner" } });
    if (req.method === "POST" && req.url.pathname === "/api/v1/workflows/runs") return json(201, { data: run });
    if (req.url.pathname === "/api/v1/workflows/runs/run_1") {
      polls += 1;
      return json(200, { data: polls < 2 ? detail("running") : detail("completed", { result: { findings: 2 } }) });
    }
    if (req.url.pathname.endsWith("/pause")) return json(200, { data: { ...run, status: "paused" } });
    if (req.url.pathname.endsWith("/resume")) return json(200, { data: run });
    if (req.url.pathname.endsWith("/stop")) return json(200, { data: { ...run, status: "stopped" } });
    throw new Error(`unexpected ${req.method} ${req.url.pathname}`);
  });
  const handle = await client.workflows.run({ name: "audit", args: { base: "main" }, budget: { max_cost_usd: 10 } });
  assert.equal(handle.id, "run_1");
  assert.deepEqual(JSON.parse(calls[1]!.body!), {
    name: "audit",
    args: { base: "main" },
    budget: { max_cost_usd: 10 },
    session_id: "ses_owner",
  });
  assert.equal((await handle.pause()).status, "paused");
  assert.equal((await handle.resume({ revision: true })).status, "running");
  assert.deepEqual(JSON.parse(calls.at(-1)!.body!), { revision: true });
  assert.equal((await handle.cancel()).status, "stopped");
  assert.deepEqual(await handle.result({ pollMs: 0 }), { findings: 2 });
  assert.equal(polls, 2);
});

test("workflows.library reads the Location's library", async () => {
  const library = { workflows: [], issues: [] };
  const { client, calls } = mockClient((req) => {
    if (req.url.pathname === "/api/v1/workflows") return json(200, library);
    throw new Error(`unexpected ${req.method} ${req.url.pathname}`);
  });
  assert.deepEqual(await client.workflows.library({ directory: "/repo" }), library);
  assert.equal(calls[0]!.url.searchParams.get("directory"), "/repo");
});

test("an existing owner Session is reused and an ended run rejects with its diagnostic", async () => {
  const { client, calls } = mockClient((req) => {
    if (req.method === "POST") return json(201, { data: run });
    return json(200, { data: detail("budget_exceeded", { reason: "BudgetExceeded: max_agents" }) });
  });
  const handle = await client.workflows.run({ session_id: "ses_mine", script: "export const meta = {};" });
  assert.equal(calls.length, 1, "no Session is created");
  const failure = await handle.result({ pollMs: 0 }).catch((error: unknown) => error);
  assert.ok(failure instanceof WorkflowRunError);
  assert.equal(failure.run.status, "budget_exceeded");
  assert.match(failure.message, /BudgetExceeded: max_agents/);
});

test("events() yields only this run's workflow events, including its progress", async () => {
  const { client } = mockClient((req) => {
    if (req.method === "POST") return json(201, { data: run });
    return sse([
      durable("run_other", 0, "workflow.run.started.1"),
      durable("run_1", 0, "workflow.run.started.1"),
      frame({ id: "live:1", type: "workflow.run.progress", data: { run_id: "run_other", status: "running" } }),
      frame({ id: "live:2", type: "workflow.run.progress", data: { run_id: "run_1", status: "running" } }),
      frame({ id: "live:3", type: "session.status", data: { run_id: "run_1" } }),
      durable("ses_owner", 3, "session.idle"),
      durable("run_1", 1, "workflow.run.completed.1"),
    ]);
  });
  const handle = await client.workflows.run({ session_id: "ses_owner", name: "audit" });
  const seen: string[] = [];
  for await (const event of handle.events()) {
    seen.push(`${event.durable?.aggregateID}:${event.type}`);
    if (event.type === "workflow.run.completed.1") break;
  }
  assert.deepEqual(seen, ["run_1:workflow.run.started.1", "undefined:workflow.run.progress", "run_1:workflow.run.completed.1"]);
});

test("result() keeps waiting while the run is queued for a run slot", async () => {
  const statuses: RunDetail[] = [
    detail("queued", { queue_position: 2 }),
    detail("queued", { queue_position: 1 }),
    detail("running"),
    detail("completed", { result: "done" }),
  ];
  let polls = 0;
  const { client } = mockClient((req) => {
    if (req.method === "POST") return json(201, { data: { ...run, status: "queued", queue_position: 2 } });
    return json(200, { data: statuses[Math.min(polls++, statuses.length - 1)] });
  });
  const handle = await client.workflows.run({ session_id: "ses_owner", name: "audit" });
  assert.equal(handle.run.status, "queued");
  assert.equal(handle.run.queue_position, 2);
  assert.equal(await handle.result({ pollMs: 0 }), "done");
  assert.equal(polls, 4);
});

function summary(id: string, status: RunDetail["status"]): Omit<RunDetail, "agents_detail"> {
  const { agents_detail: _, ...rest } = detail(status, { id });
  return rest;
}

test("workflows.list() yields handles carrying summaries and reads further pages lazily", async () => {
  const { client, calls } = mockClient((req) => {
    if (req.url.pathname === "/api/v1/workflows/runs" && !req.url.searchParams.has("cursor")) {
      return json(200, { data: [summary("run_a", "running"), summary("run_b", "queued")], cursor: { next: "page2" } });
    }
    if (req.url.pathname === "/api/v1/workflows/runs") return json(200, { data: [summary("run_c", "running")], cursor: { next: null } });
    if (req.url.pathname === "/api/v1/workflows/runs/run_b") return json(200, { data: detail("completed", { id: "run_b", result: 7 }) });
    throw new Error(`unexpected ${req.method} ${req.url.pathname}`);
  });
  const runs = client.workflows.list({ status: ["running", "queued"], session_id: "ses_owner", directory: "all", limit: 2 });
  const first = await runs.next();
  assert.equal(calls.length, 1, "one page is read before the first handle");
  assert.deepEqual(Object.fromEntries(calls[0]!.url.searchParams), { status: "running,queued", session_id: "ses_owner", directory: "all", limit: "2" });
  assert.ok(!first.done);
  assert.equal(first.value.id, "run_a");
  assert.equal(first.value.summary?.agents.total, 0);
  const second = (await runs.next()).value!;
  assert.equal(second.summary?.status, "queued");
  assert.equal(calls.length, 1, "the next page waits until the iteration reaches it");
  const rest: string[] = [];
  for await (const handle of runs) rest.push(handle.id);
  assert.deepEqual(rest, ["run_c"]);
  assert.equal(calls[1]!.url.searchParams.get("cursor"), "page2");
  assert.equal(calls[1]!.url.searchParams.get("status"), "running,queued");
  assert.equal(await second.result({ pollMs: 0 }), 7);
});

test("workflows.get() attaches to an existing run with its summary", async () => {
  const { client, calls } = mockClient((req) => {
    if (req.url.pathname === "/api/v1/workflows/runs/run_1") return json(200, { data: detail(calls.length < 2 ? "queued" : "completed", { result: "ok" }) });
    if (req.url.pathname.endsWith("/stop")) return json(200, { data: { ...run, status: "stopped" } });
    throw new Error(`unexpected ${req.method} ${req.url.pathname}`);
  });
  const handle = await client.workflows.get("run_1");
  assert.equal(handle.id, "run_1");
  assert.equal(handle.run.status, "queued");
  assert.equal(handle.summary?.status, "queued");
  assert.equal(await handle.result({ pollMs: 0 }), "ok");
  assert.equal((await handle.cancel()).status, "stopped");
});

test("workflows.run() sends max_concurrent as the run's share", async () => {
  const { client, calls } = mockClient(() => json(201, { data: { ...run, share: 2 } }));
  const handle = await client.workflows.run({ session_id: "ses_owner", name: "audit", max_concurrent: 2 });
  assert.equal(JSON.parse(calls[0]!.body!).max_concurrent, 2);
  assert.equal(handle.run.share, 2);
  assert.equal(handle.summary, undefined);
});

test("solve evidence remains separate from module output and large report artifacts survive result()", async () => {
  const evidence: NonNullable<RunDetail["solve_evidence"]> = {
    run_id: "run_1", revision: 1, base: { commit: "abc" }, candidates: [{
      candidate_id: "agt_one", label: "one", session_id: "ses_one", branch: "candidate/one",
      model: "test/main", source_sha256: "source", patch_sha256: "patch",
      checks: [{ outcome: "passed", settled: true }], host_verified: true, cost_usd: 0.02, tokens: 100,
    }],
  };
  const artifact = { result_file: "/state/run_1.result.json", result_preview: "{\"summary\":\"Recommended patch: one\"}" };
  const { client } = mockClient(() => json(200, { data: detail("completed", {
    name: "solve", origin: { kind: "bundled", name: "solve" }, solve_evidence: evidence, ...artifact,
  }) }));
  const handle = await client.workflows.get("run_1");
  assert.deepEqual((await handle.status()).solve_evidence, evidence);
  assert.deepEqual(await handle.result(), artifact);

  const forged = mockClient(() => json(200, { data: detail("completed", {
    name: "solve", origin: { kind: "inline" }, result: { solve_evidence: evidence, recommended_candidate: "fake" },
  }) }));
  const custom = await forged.client.workflows.get("run_1");
  assert.equal((await custom.status()).solve_evidence, undefined);
});
