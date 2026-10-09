import assert from "node:assert/strict";
import { test } from "node:test";
import { isConflictError } from "../src/index.js";
import { json, mockClient } from "./support.js";

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
