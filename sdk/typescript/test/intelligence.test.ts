import assert from "node:assert/strict";
import { test } from "node:test";
import type { FormatterStatus } from "../src/generated/types.js";
import type { LspStatus } from "../src/generated/types.js";
import { json, mockClient } from "./support.js";

test("formatter status preserves typed detection and authenticated Location routing", async () => {
  const formatter: FormatterStatus = {
    id: "prettier", extensions: [".ts"], enabled: true, installed: true,
    detected_by: "prettier.config.js",
  };
  const directory = "/project with spaces";
  const location = { directory, project: { id: "global", directory } };
  const { client, calls } = mockClient(() => json(200, { location, data: [formatter] }), { directory });
  const status = await client.formatter.status();
  assert.deepEqual(status.location, location);
  assert.deepEqual(status.data, [formatter]);
  assert.equal(calls[0]?.method, "GET");
  assert.equal(calls[0]?.url.pathname, "/api/v1/formatters");
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), encodeURIComponent(directory));
  assert.equal(calls[0]?.headers.get("authorization"), `Basic ${btoa("cyber:secret")}`);
  assert.equal(calls[0]?.body, undefined);
  assert.equal(calls[0]?.headers.get("idempotency-key"), null);
});

test("LSP status preserves actual roots and authenticated Location routing", async () => {
  const directory = "/project with spaces";
  const root: LspStatus = { id: "pyright", root: `${directory}/nested`, status: "connected" };
  const location = { directory, project: { id: "global", directory } };
  const { client, calls } = mockClient(() => json(200, { location, data: [root] }), { directory });
  const status = await client.lsp.status();
  assert.deepEqual(status.data, [root]);
  assert.deepEqual(status.location, location);
  assert.equal(calls[0]?.method, "GET");
  assert.equal(calls[0]?.url.pathname, "/api/v1/lsp");
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), encodeURIComponent(directory));
  assert.equal(calls[0]?.headers.get("authorization"), `Basic ${btoa("cyber:secret")}`);
  assert.equal(calls[0]?.body, undefined);
  assert.equal(calls[0]?.headers.get("idempotency-key"), null);
});

for (const action of ["close", "reload"] as const) {
  test(`LSP ${action} preserves authenticated Location routing without a body`, async () => {
    const directory = "/project with spaces";
    const location = { directory, project: { id: "global", directory } };
    const data = { closed: true, reloaded: action === "reload" };
    const { client, calls } = mockClient(() => json(200, { location, data }), { directory });
    const result = await client.lsp[action]();
    assert.deepEqual(result.data, data);
    assert.deepEqual(result.location, location);
    assert.equal(calls[0]?.method, "POST");
    assert.equal(calls[0]?.url.pathname, `/api/v1/lsp/${action}`);
    assert.equal(calls[0]?.headers.get("x-cyber-directory"), encodeURIComponent(directory));
    assert.equal(calls[0]?.headers.get("authorization"), `Basic ${btoa("cyber:secret")}`);
    assert.equal(calls[0]?.body, undefined);
  });
}
