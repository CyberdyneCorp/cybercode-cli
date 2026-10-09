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
