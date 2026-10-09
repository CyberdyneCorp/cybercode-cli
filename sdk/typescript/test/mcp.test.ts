import assert from "node:assert/strict";
import { test } from "node:test";
import { isConflictError } from "../src/index.js";
import type { McpStatusUpdate } from "../src/index.js";
import { frame, sse, json, mockClient } from "./support.js";

test("MCP close sends authenticated Location-scoped POST without a body", async () => {
  const directory = "/checkout A/nested";
  const location = { directory, project: { id: "global", directory: "/checkout A" } };
  const { client, calls } = mockClient(() => json(200, { location, data: { closed: true } }), { directory });
  const result = await client.mcp.close();
  assert.equal(result.data.closed, true);
  assert.deepEqual(result.location, location);
  assert.equal(calls[0]?.method, "POST");
  assert.equal(calls[0]?.url.pathname, "/api/v1/mcp/close");
  assert.equal(calls[0]?.body, undefined);
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), encodeURIComponent(directory));
  assert.equal(calls[0]?.headers.get("authorization"), `Basic ${btoa("cyber:secret")}`);
});

test("MCP unknown ownership is a conflict without automatic close replay", async () => {
  const { client, calls } = mockClient(() => json(409, { _tag: "ConflictError", message: "MCP Location close requires retry or recovery" }));
  await assert.rejects(client.mcp.close(), isConflictError);
  assert.equal(calls.length, 1);
});


test("MCP status uses its own durable aggregate on the Location event stream", async () => {
  const update: McpStatusUpdate = {
    connection_id: "mcs_local", directory: "/repo", name: "local",
    status: "failed", phase: "unknown", acknowledged: false,
    error: "MCP native settlement is unverified",
  };
  const { client, calls } = mockClient(() => sse([frame({
    id: "mcs_local:4", type: "mcp.status.changed.1", data: update,
    durable: { aggregateID: "mcs_local", seq: 4, version: 1 },
  })]), { directory: "/repo" });
  for await (const event of client.events.subscribe()) {
    assert.equal(event.type, "mcp.status.changed.1");
    assert.equal(event.durable?.aggregateID, "mcs_local");
    assert.equal(event.durable?.seq, 4);
    assert.deepEqual(event.data, update);
    break;
  }
  assert.equal(calls[0]?.url.pathname, "/api/v1/event");
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), encodeURIComponent("/repo"));
});
