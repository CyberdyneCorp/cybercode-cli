import assert from "node:assert/strict";
import { test } from "node:test";
import { CyberClientError } from "../src/index.js";
import { json, mockClient } from "./support.js";

const UUID_V7 = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const location = { directory: "/r", project: { id: "p", directory: "/r" } };

test("transport failures are retried with the same Idempotency-Key", async () => {
  const { client, calls } = mockClient((_req, i) => {
    if (i < 2) throw new TypeError("fetch failed");
    return json(201, { location, data: { id: "ses_1" } });
  });
  const created = await client.session.create({ title: "t" });
  assert.equal(created.data.id, "ses_1");
  assert.equal(calls.length, 3);
  const keys = calls.map((c) => c.headers.get("idempotency-key"));
  assert.match(keys[0] ?? "", UUID_V7);
  assert.deepEqual(new Set(keys).size, 1);
  assert.deepEqual(calls.map((c) => c.body), Array(3).fill('{"title":"t"}'));
});

test("gives up after 3 retries with reason Transport", async () => {
  const { client, calls } = mockClient(() => {
    throw new TypeError("connection refused");
  });
  const err = await client.session.interrupt("ses_1").catch((e: unknown) => e);
  assert.ok(err instanceof CyberClientError);
  assert.equal(err.reason, "Transport");
  assert.equal(calls.length, 4);
});

test("a caller-supplied key is sent as is; each call gets a fresh key otherwise", async () => {
  const { client, calls } = mockClient(() => new Response(null, { status: 204 }));
  await client.session.interrupt("ses_1", { idempotencyKey: "my-key" });
  await client.session.interrupt("ses_1");
  await client.session.interrupt("ses_1");
  assert.equal(calls[0]?.headers.get("idempotency-key"), "my-key");
  assert.notEqual(calls[1]?.headers.get("idempotency-key"), calls[2]?.headers.get("idempotency-key"));
});

test("GET requests carry no key and API errors are not retried", async () => {
  const { client, calls } = mockClient(() => json(404, { _tag: "SessionNotFoundError", message: "missing" }));
  await client.session.get("ses_1").catch(() => {});
  await client.session.interrupt("ses_1").catch(() => {});
  assert.equal(calls.length, 2);
  assert.equal(calls[0]?.headers.get("idempotency-key"), null);
});
