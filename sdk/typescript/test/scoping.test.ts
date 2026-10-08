import assert from "node:assert/strict";
import { test } from "node:test";
import { Cyber } from "../src/index.js";
import { json, mockClient } from "./support.js";

const location = { directory: "/r", project: { id: "p", directory: "/r" } };

test("at() scopes Location requests without mutating the parent", async () => {
  const { client, calls } = mockClient(() => json(200, { location, data: [] }), { directory: "/repo/a" });
  const b = client.at("/repo/b c");
  await client.agent.list();
  await b.agent.list();
  await client.agent.list();
  const sent = calls.map((c) => c.headers.get("x-cyber-directory"));
  assert.deepEqual(sent, ["%2Frepo%2Fa", "%2Frepo%2Fb%20c", "%2Frepo%2Fa"]);
  assert.equal(client.directory, "/repo/a");
  assert.equal(b.directory, "/repo/b c");
});

test("non-Location routes carry no directory; every request is authenticated", async () => {
  const { client, calls } = mockClient(() => json(200, { data: { id: "ses_1" } }), { directory: "/repo/a" });
  await client.at("/repo/b").session.get("ses_1");
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), null);
  assert.equal(calls[0]?.headers.get("authorization"), `Basic ${btoa("cyber:secret")}`);
  assert.equal(calls[0]?.url.pathname, "/api/v1/sessions/ses_1");
});

test("query parameters and path parameters are encoded", async () => {
  const { client, calls } = mockClient(() => json(200, { data: [], cursor: {} }));
  await client.message.list("ses/1", { limit: 5, order: "desc", cursor: undefined });
  assert.equal(calls[0]?.url.pathname, "/api/v1/sessions/ses%2F1/messages");
  assert.equal(calls[0]?.url.search, "?limit=5&order=desc");
});

test("account auth sends a Bearer token from the provider", async () => {
  let n = 0;
  const { client, calls } = mockClient(() => json(200, { healthy: true }), {
    auth: { type: "account", tokenProvider: () => `tok${++n}` },
  });
  await client.health.get();
  await client.health.get();
  assert.deepEqual(calls.map((c) => c.headers.get("authorization")), ["Bearer tok1", "Bearer tok2"]);
});

test("connect() does no I/O; embedded mode reports a missing binary", async () => {
  let fetched = false;
  Cyber.connect({ fetch: async () => ((fetched = true), new Response()) });
  assert.equal(fetched, false);
  await assert.rejects(Cyber.start({ mode: "embedded", binary: "/nonexistent/cyber" }), /cannot start `\/nonexistent\/cyber serve --stdio`/);
});

test("session.subtask posts an explicit prompt and returns the background Job", async () => {
  const job = { id: "job_1", child_id: "ses_child", session_id: "ses/parent", name: "build", status: "running" };
  const { client, calls } = mockClient(() => json(202, { data: job }));
  const result = await client.session.subtask("ses/parent", { prompt: "try another approach" });
  assert.equal(result.id, "job_1");
  assert.equal(result.child_id, "ses_child");
  assert.equal(calls[0]?.method, "POST");
  assert.equal(calls[0]?.url.pathname, "/api/v1/sessions/ses%2Fparent/subtask");
  assert.deepEqual(JSON.parse(calls[0]!.body!), { prompt: "try another approach" });
});

test("child setup inspection scopes both identities without a Location header", async () => {
  const state = { session_id: "ses_child", worktree_id: "wt_child", setup_pending: true,
    journal: { revision: 3, digest: "recipe", commands: [{ status: "finished", result: { status: "exited", code: 7 } }] } };
  const { client, calls } = mockClient(() => json(200, { data: state }), { directory: "/other" });
  const inspected = await client.worktree.inspectChildSetup("ses/parent", "child/name");
  assert.equal(inspected.journal.revision, 3);
  assert.equal(calls[0]?.url.pathname, "/api/v1/sessions/ses%2Fparent/children/child%2Fname/setup");
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), null);
});

test("explicit child setup recovery sends the reviewed revision and reuses an idempotency key", async () => {
  const { client, calls } = mockClient(() => json(200, { data: { setup: { status: "completed" } } }));
  const review = { revision: 3, digest: "recipe", retry_index: 1, reason: "Prerequisite repaired" };
  await client.worktree.recoverChildSetup("ses_parent", "ses_child", review, { idempotencyKey: "retry-review" });
  assert.equal(calls[0]?.method, "POST");
  assert.equal(calls[0]?.url.pathname, "/api/v1/sessions/ses_parent/children/ses_child/setup");
  assert.equal(calls[0]?.headers.get("idempotency-key"), "retry-review");
  assert.deepEqual(JSON.parse(calls[0]!.body!), review);
});


test("named user delegation sends the selected profile through the Session endpoint", async () => {
  const { client, calls } = mockClient(() => json(202, { data: {
    id: "job_named", child_id: "ses_child", session_id: "ses_parent", name: "explore", status: "running",
  } }));
  const result = await client.session.subtask("ses_parent", { prompt: "find retry logic", agent: "explore" });
  assert.equal(result.name, "explore");
  assert.deepEqual(JSON.parse(calls[0]!.body!), { prompt: "find retry logic", agent: "explore" });
  assert.equal(calls[0]?.url.pathname, "/api/v1/sessions/ses_parent/subtask");
});

test("typed delegation retains image attachments and the reviewed turn ceiling", async () => {
  const { client, calls } = mockClient(() => json(202, { data: {
    id: "job_named", child_id: "ses_child", session_id: "ses_parent", status: "running",
  } }));
  const body = {
    prompt: "inspect image", agent: "explore", max_steps: 2,
    attachments: [{ type: "image" as const, media_type: "image/png", data: "aW1hZ2U=" }],
  };
  await client.session.subtask("ses_parent", body, { idempotencyKey: "typed-child" });
  assert.deepEqual(JSON.parse(calls[0]!.body!), body);
  assert.equal(calls[0]?.headers.get("idempotency-key"), "typed-child");
});

test("durable delegation start, lookup and stop share a scoped client-selected identity", async () => {
  const { client, calls } = mockClient(() => json(202, { data: {
    id: "op_owned", session_id: "ses/parent", status: "pending", phase: "reserved",
    job_id: null, error: null,
  } }));
  const body = { prompt: "inspect project", agent: "explore", max_steps: 2 };
  const started = await client.session.startDelegation("ses/parent", "op_owned", body);
  assert.equal(started.id, "op_owned");
  await client.session.delegation("ses/parent", "op_owned");
  await client.session.stopDelegation("ses/parent", "op_owned", { idempotencyKey: "stop-owned" });
  assert.deepEqual(calls.map(call => [call.method, call.url.pathname]), [
    ["POST", "/api/v1/sessions/ses%2Fparent/delegations/op_owned"],
    ["GET", "/api/v1/sessions/ses%2Fparent/delegations/op_owned"],
    ["POST", "/api/v1/sessions/ses%2Fparent/delegations/op_owned/stop"],
  ]);
  assert.deepEqual(JSON.parse(calls[0]!.body!), body);
  assert.equal(calls[2]?.headers.get("idempotency-key"), "stop-owned");
});

test("usage.get preserves scope identity and incomplete billing without a Location override", async () => {
  const amount = { tokens: { input: 10, output: 2, reasoning: 3, cache_read: 4, cache_write: 5 },
    total_tokens: 24, cost: 0.25, unpriced_steps: 1, usage_complete: false, token_classes_complete: false };
  const report = { scope: "session", id: "ses/parent", own: amount, descendants: amount, total: amount };
  const { client, calls } = mockClient(() => json(200, { data: report }), { directory: "/other" });
  const result = await client.usage.get({ scope: "session", id: "ses/parent" });
  assert.equal(result.total.token_classes_complete, false);
  assert.equal(result.total.unpriced_steps, 1);
  assert.equal(calls[0]?.url.pathname, "/api/v1/usage");
  assert.equal(calls[0]?.url.searchParams.get("id"), "ses/parent");
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), null);
  assert.ok(calls[0]?.headers.get("authorization"));
});

test("session.children preserves parent identity and cursor without Location scoping", async () => {
  const page = { parent_id: "ses/parent", data: [], cursor: { previous: null, next: "opaque /+" } };
  const { client, calls } = mockClient(() => json(200, { data: page }), { directory: "/repo/a" });
  const result = await client.at("/repo/other").session.children("ses/parent", { limit: 2, cursor: "opaque /+" });
  assert.deepEqual(result, page);
  assert.equal(calls[0]?.url.pathname, "/api/v1/sessions/ses%2Fparent/children");
  assert.equal(calls[0]?.url.searchParams.get("cursor"), "opaque /+");
  assert.equal(calls[0]?.url.searchParams.get("limit"), "2");
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), null);
  assert.equal(calls[0]?.headers.get("authorization"), `Basic ${btoa("cyber:secret")}`);
});

test("session.wake dispatches existing input without a prompt body or Location scope", async () => {
  const { client, calls } = mockClient(() => new Response(null, { status: 204 }), { directory: "/repo/a" });
  assert.equal(await client.at("/repo/b").session.wake("ses/child"), undefined);
  assert.equal(calls[0]?.url.pathname, "/api/v1/sessions/ses%2Fchild/wake");
  assert.equal(calls[0]?.method, "POST");
  assert.equal(calls[0]?.body, undefined);
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), null);
  assert.equal(calls[0]?.headers.get("authorization"), `Basic ${btoa("cyber:secret")}`);
});
