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
