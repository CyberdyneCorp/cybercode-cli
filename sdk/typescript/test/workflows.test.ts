import assert from "node:assert/strict";
import { test } from "node:test";
import { WorkflowRunError, type RunDetail, type WorkflowRun } from "../src/index.js";
import { durable, json, mockClient, sse } from "./support.js";

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
  return { ...run, status, agents: [], ...extra };
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

test("events() yields only this run's durable events", async () => {
  const { client } = mockClient((req) => {
    if (req.method === "POST") return json(201, { data: run });
    return sse([
      durable("run_other", 0, "workflow.run.started.1"),
      durable("run_1", 0, "workflow.run.started.1"),
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
  assert.deepEqual(seen, ["run_1:workflow.run.started.1", "run_1:workflow.run.completed.1"]);
});
