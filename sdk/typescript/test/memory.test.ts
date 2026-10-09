import assert from "node:assert/strict";
import { test } from "node:test";
import { isConflictError, isMemoryNotFoundError } from "../src/index.js";
import type { MemoryChange } from "../src/index.js";
import { json, mockClient, sse, frame } from "./support.js";

test("memory review preserves authenticated scope and Location routing", async () => {
  const location = { directory: "/repo A", project: { id: "prj_test", directory: "/repo A" } };
  const { client, calls } = mockClient(() => json(200, { location, data: { memories: [{ name: "policy", description: "Durable policy", type: "reference" }], invalid: [] } }), { directory: location.directory });
  const catalog = await client.memory.list({ scope: "global" });
  assert.equal(catalog.data.memories[0]?.name, "policy");
  assert.deepEqual(catalog.location, location);
  assert.equal(calls[0]?.method, "GET");
  assert.equal(calls[0]?.url.pathname, "/api/v1/memory");
  assert.equal(calls[0]?.url.searchParams.get("scope"), "global");
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), encodeURIComponent(location.directory));
  assert.equal(calls[0]?.headers.get("authorization"), `Basic ${btoa("cyber:secret")}`);
  assert.equal(calls[0]?.headers.get("idempotency-key"), null);
});

test("memory note review returns typed document fields", async () => {
  const { client, calls } = mockClient(() => json(200, { location: { directory: "/repo", project: { id: "global", directory: "/repo" } }, data: { metadata: { name: "policy", description: "Durable policy", type: "feedback" }, body: "**Why:** predictable changes\n**How to apply:** small commits" } }));
  const note = await client.memory.get("project", "policy");
  assert.equal(note.data.metadata.type, "feedback");
  assert.ok(note.data.body.includes("**Why:**"));
  assert.equal(calls[0]?.url.pathname, "/api/v1/memory/project/policy");
});

test("memory conflicts and missing notes use tagged errors without automatic replay", async () => {
  for (const [status, tag, guard] of [[409, "ConflictError", isConflictError], [404, "MemoryNotFoundError", isMemoryNotFoundError]] as const) {
    const { client, calls } = mockClient(() => json(status, { _tag: tag, message: "Memory unavailable" }));
    await assert.rejects(client.memory.get("global", "policy"), guard);
    assert.equal(calls.length, 1);
  }
});


test("memory updates carry independent durable receipt identity without note content", async () => {
  const update: MemoryChange = { id: "mwr_request", directory: "/repo", project_id: "global", receipt: { id: "mem_committed", name: "policy", deleted: false } };
  const { client } = mockClient(() => sse([frame({ id: "mwr_request:1", type: "memory.updated.1", data: update, location: "/repo", durable: { aggregateID: "mwr_request", seq: 1, version: 1 } })]), { directory: "/repo" });
  for await (const event of client.events.subscribe()) {
    assert.equal(event.type, "memory.updated.1");
    assert.equal(event.durable?.aggregateID, update.id);
    assert.equal(event.durable?.seq, 1);
    assert.deepEqual(event.data, update);
    break;
  }
});

test("memory writes send complete documents and stable authenticated request identity", async () => {
  const change: MemoryChange = { id: "mwr_request", directory: "/repo", project_id: "prj_test", receipt: { id: "mem_committed", name: "policy", deleted: false } };
  const { client, calls } = mockClient(() => json(200, { location: { directory: "/repo", project: { id: "prj_test", directory: "/repo" } }, data: change }), { directory: "/repo" });
  const content = "---\nname: policy\ndescription: Durable policy\ntype: reference\n---\nDurable fact\n";
  const result = await client.memory.put("project", "policy", { content }, { idempotencyKey: "memory-write" });
  assert.deepEqual(result.data, change);
  assert.equal(calls[0]?.method, "PUT");
  assert.equal(calls[0]?.url.pathname, "/api/v1/memory/project/policy");
  assert.deepEqual(JSON.parse(calls[0]?.body ?? "null"), { content });
  assert.equal(calls[0]?.headers.get("idempotency-key"), "memory-write");
  assert.equal(calls[0]?.headers.get("authorization"), `Basic ${btoa("cyber:secret")}`);
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), encodeURIComponent("/repo"));
});

test("memory deletes send no body and preserve the acknowledged deletion receipt", async () => {
  const change: MemoryChange = { id: "mwr_request", directory: "/repo", project_id: "global", receipt: { id: "mem_committed", name: "policy", deleted: true } };
  const { client, calls } = mockClient(() => json(200, { location: { directory: "/repo", project: { id: "global", directory: "/repo" } }, data: change }));
  const result = await client.memory.delete("global", "policy", { idempotencyKey: "memory-delete" });
  assert.deepEqual(result.data, change);
  assert.equal(calls[0]?.method, "DELETE");
  assert.equal(calls[0]?.url.pathname, "/api/v1/memory/global/policy");
  assert.equal(calls[0]?.body, undefined);
  assert.equal(calls[0]?.headers.get("idempotency-key"), "memory-delete");
});

test("memory recovery inspection preserves nullable review and Location", async () => {
  const { client, calls } = mockClient(() => json(200, { location: { directory: "/repo", project: { id: "global", directory: "/repo" } }, data: null }), { directory: "/repo" });
  const result = await client.memory.recovery("global");
  assert.equal(result.data, null);
  assert.equal(calls[0]?.method, "GET");
  assert.equal(calls[0]?.url.pathname, "/api/v1/memory/recovery/global");
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), encodeURIComponent("/repo"));
  assert.equal(calls[0]?.headers.get("idempotency-key"), null);
});

test("memory recovery confirms both fingerprints and refuses stale review without retry", async () => {
  const { client, calls } = mockClient(() => json(409, { _tag: "ConflictError", message: "Memory recovery review is stale" }), { directory: "/repo" });
  const review = { storage_fingerprint: "a".repeat(64), admission_fingerprint: `sha256:${"b".repeat(64)}` };
  await assert.rejects(client.memory.recover("project", review, { idempotencyKey: "reviewed-recovery" }), isConflictError);
  assert.equal(calls.length, 1);
  assert.equal(calls[0]?.method, "POST");
  assert.equal(calls[0]?.url.pathname, "/api/v1/memory/recovery/project");
  assert.deepEqual(JSON.parse(calls[0]?.body ?? "null"), review);
  assert.equal(calls[0]?.headers.get("idempotency-key"), "reviewed-recovery");
  assert.equal(calls[0]?.headers.get("authorization"), `Basic ${btoa("cyber:secret")}`);
});

test("memory recovery exposes typed paired journal and admission evidence", async () => {
  const receipt = { id: "mem_prepared", name: "policy", deleted: false };
  const journal = { receipt, intent_fingerprint: "c".repeat(64) };
  const view: import("../src/index.js").MemoryRecoveryView = {
    storage: { receipt, journal, completed: false, fingerprint: "a".repeat(64), proposed_note: { metadata: { name: "policy", description: "Policy", type: "reference" }, body: "Preference" } },
    admission: { id: "mwr_pending", directory: "/repo", project_id: "global", journal, sequence: 1, fingerprint: `sha256:${"b".repeat(64)}`, completed: null },
  };
  const { client } = mockClient(() => json(200, { location: { directory: "/repo", project: { id: "global", directory: "/repo" } }, data: view }));
  const result = await client.memory.recovery("global");
  assert.deepEqual(result.data, view);
  assert.equal(result.data?.storage.proposed_note?.body, "Preference");
  assert.deepEqual(result.data?.storage.journal, result.data?.admission.journal);
});

test("retained recovery request lookup preserves key encoding and unresolved evidence", async () => {
  const key = "recovery/request+key";
  const status: import("../src/index.js").MemoryRecoveryRequestStatus = {
    id: "mrr_key", mutation_id: "mwr_pending", directory: "/repo", project_id: "global",
    journal: { receipt: { id: "mem_prepared", name: "policy", deleted: false }, intent_fingerprint: "a".repeat(64) }, completed: null,
  };
  const { client, calls } = mockClient(() => json(200, { location: { directory: "/repo", project: { id: "global", directory: "/repo" } }, data: status }), { directory: "/repo" });
  const result = await client.memory.recoveryRequest({ scope: "global", key });
  assert.deepEqual(result.data, status);
  assert.equal(result.data?.completed, null);
  assert.equal(calls[0]?.method, "GET");
  assert.equal(calls[0]?.url.pathname, "/api/v1/memory/recovery/requests");
  assert.equal(calls[0]?.url.searchParams.get("key"), key);
  assert.equal(calls[0]?.url.searchParams.get("scope"), "global");
  assert.equal(calls[0]?.headers.get("idempotency-key"), null);
  assert.equal(calls.length, 1);
});

test("memory edit review returns original Markdown and a typed conditional fingerprint", async () => {
  const review: import("../src/index.js").MemoryEditReview = { name: "policy", original: "original Markdown", fingerprint: "a".repeat(64) };
  const { client, calls } = mockClient(() => json(200, { location: { directory: "/repo", project: { id: "global", directory: "/repo" } }, data: review }), { directory: "/repo" });
  const result = await client.memory.editReview("global", "policy");
  assert.deepEqual(result.data, review);
  assert.equal(calls[0]?.url.pathname, "/api/v1/memory/edit/global/policy");
  assert.equal(calls[0]?.method, "GET");
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), encodeURIComponent("/repo"));
  assert.equal(calls[0]?.headers.get("idempotency-key"), null);
});

test("conditional memory mutations carry reviewed fingerprints and refuse stale responses once", async () => {
  const { client, calls } = mockClient(() => json(409, { _tag: "ConflictError", message: "Memory edit review is stale" }), { directory: "/repo" });
  const fingerprint = "b".repeat(64);
  await assert.rejects(client.memory.put("global", "policy", { content: "draft", review_fingerprint: fingerprint }, { idempotencyKey: "reviewed-save" }), isConflictError);
  assert.deepEqual(JSON.parse(calls[0]?.body ?? "null"), { content: "draft", review_fingerprint: fingerprint });
  assert.equal(calls[0]?.headers.get("idempotency-key"), "reviewed-save");
  await assert.rejects(client.memory.delete("global", "policy", { idempotencyKey: "reviewed-delete", headers: { "x-cyber-memory-review": fingerprint } }), isConflictError);
  assert.equal(calls.length, 2);
  assert.equal(calls[1]?.headers.get("x-cyber-memory-review"), fingerprint);
  assert.equal(calls[1]?.headers.get("idempotency-key"), "reviewed-delete");
  assert.equal(calls[1]?.body, undefined);
});
