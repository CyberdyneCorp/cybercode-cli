import assert from "node:assert/strict";
import { test } from "node:test";
import { CyberApiError, isCollaborationQueued } from "../src/index.js";
import { json, mockClient } from "./support.js";

test("local helper preserves checkout and fences; an unknown transport outcome retries one operation", async () => {
  let attempts = 0;
  const { client, calls } = mockClient(() => {
    if (attempts++ === 0) throw new TypeError("fixture disconnect");
    return json(200, { data: { id: "ctask_one", revision: 8, claim_generation: 4 } });
  }, { directory: "/work/clone" });
  const handle = client.collaborations.forSession("ses_owner", "cprj_one");
  const result = await handle.update({ id: "ctask_one", revision: 7, claim_generation: 4 }, { status: "done" }, { operationId: "cop_stable" });
  assert.ok(!isCollaborationQueued(result));
  assert.equal(result.revision, 8);
  assert.equal(calls.length, 2);
  assert.equal(calls[0]!.body, calls[1]!.body);
  assert.equal(calls[0]!.headers.get("x-cyber-directory"), encodeURIComponent("/work/clone"));
  assert.deepEqual(JSON.parse(calls[0]!.body!), { session_id: "ses_owner", project: "cprj_one", status: "done", expected_revision: 7, claim_generation: 4, operation_id: "cop_stable" });
  assert.equal(calls[0]!.url.pathname, "/api/v1/collaboration/tasks/ctask_one/update");
});

test("disabled snapshots make no remote roster/task requests and watches stop on revocation", async () => {
  const { client, calls } = mockClient(() => json(200, { data: { enabled: false, binding: null } }));
  assert.deepEqual((await client.collaborations.forSession("ses_owner").snapshot()).tasks, []);
  assert.equal(calls.length, 1);
  let snapshot = 0;
  const revoked = mockClient((req) => {
    if (req.url.pathname.endsWith("/status")) {
      if (snapshot++ > 0) return json(403, { _tag: "ForbiddenError", message: "membership revoked" });
      return json(200, { data: { enabled: true, binding: { project: "cprj_one" } } });
    }
    return json(200, { data: req.url.pathname.endsWith("/participants") ? { members: [], agents: [] } : [] });
  });
  const watch = revoked.client.collaborations.forSession("ses_owner").watch({ intervalMs: 250 });
  assert.deepEqual((await watch.next()).value?.messages, []);
  await assert.rejects(watch.next(), (error) => error instanceof CyberApiError && error.status === 403);
  assert.equal((await watch.next()).done, true);
});

test("task claim conflict is surfaced without a hidden reread or takeover", async () => {
  const { client, calls } = mockClient(() => json(409, { _tag: "RevisionConflictError", message: "already claimed", current_revision: 2 }));
  await assert.rejects(client.collaborations.forSession("ses_owner").claim({ id: "ctask_one", revision: 1 }), (error) => error instanceof CyberApiError && error.status === 409);
  assert.equal(calls.length, 1);
  const payload = JSON.parse(calls[0]!.body!);
  assert.equal(payload.expected_revision, 1);
  assert.equal(payload.author, undefined);
  assert.equal(payload.assignee, undefined);
});

test("generated hub operations use a separate bearer client and retain mutation replay evidence", async () => {
  const { client, calls } = mockClient(() => json(200, { data: { id: "ctask_one", revision: 1 }, replayed: true }), { auth: { type: "account", token: "fixture-only-access" } });
  const response = await client.hub.createTask("cprj_one", { operation_id: "cop_same", payload: { title: "Verify", description: "" } });
  assert.equal(response.replayed, true);
  assert.equal(response.data.id, "ctask_one");
  assert.equal(calls[0]!.headers.get("authorization"), "Bearer fixture-only-access");
  assert.equal(calls[0]!.url.pathname, "/api/v1/hub/projects/cprj_one/tasks");
});

test("a durable queued mutation is distinct from an applied task and keeps its receipt", async () => {
  const { client } = mockClient(() => json(200, { data: { status: "queued", operation_id: "cop_pending", message: "Durably queued; do not resend" } }));
  const result = await client.collaborations.forSession("ses_owner").createTask({ title: "Pending", description: "" });
  assert.ok(isCollaborationQueued(result));
  assert.equal(result.operation_id, "cop_pending");
});
