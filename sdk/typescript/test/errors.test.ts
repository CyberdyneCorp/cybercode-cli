import assert from "node:assert/strict";
import { test } from "node:test";
import { CyberApiError, CyberClientError, isBudgetExceededError, isSessionBusyError, isSessionNotFoundError, isRewindConflictError } from "../src/index.js";
import { json, mockClient } from "./support.js";

test("tagged error bodies reject with CyberApiError", async () => {
  const { client } = mockClient(() => json(404, { _tag: "SessionNotFoundError", message: "session ses_1 not found" }));
  const err = await client.session.get("ses_1").catch((e: unknown) => e);
  assert.ok(err instanceof CyberApiError);
  assert.equal(err.tag, "SessionNotFoundError");
  assert.equal(err.status, 404);
  assert.equal(err.body.message, "session ses_1 not found");
  assert.ok(isSessionNotFoundError(err));
  assert.ok(!isSessionBusyError(err));
});

test("generated guards discriminate on the tag", async () => {
  const { client } = mockClient(() => json(409, { _tag: "RewindConflictError", message: "conflict", paths: ["a.txt"] }));
  const err = await client.session.revertCommit("ses_1").catch((e: unknown) => e);
  assert.ok(isRewindConflictError(err));
  assert.deepEqual(err.body.paths, ["a.txt"]);
});

test("busy sessions reject with SessionBusyError", async () => {
  const { client } = mockClient(() => json(409, { _tag: "SessionBusyError", message: "busy" }));
  const err = await client.session.revertCommit("ses_1").catch((e: unknown) => e);
  assert.ok(isSessionBusyError(err));
});

test("an untagged error status is UnexpectedStatus", async () => {
  const { client } = mockClient(() => new Response("bad gateway", { status: 502 }));
  const err = await client.health.get().catch((e: unknown) => e);
  assert.ok(err instanceof CyberClientError);
  assert.equal(err.reason, "UnexpectedStatus");
  assert.equal(err.status, 502);
});

test("invalid JSON or a missing envelope is MalformedResponse", async () => {
  const { client } = mockClient((_req, i) => (i === 0 ? new Response("{nope", { status: 200 }) : json(200, { nope: 1 })));
  const first = await client.health.get().catch((e: unknown) => e);
  assert.ok(first instanceof CyberClientError && first.reason === "MalformedResponse");
  const second = await client.session.get("ses_1").catch((e: unknown) => e);
  assert.ok(second instanceof CyberClientError && second.reason === "MalformedResponse");
});

test("{ data } envelopes are unwrapped; Located and Page bodies are kept whole; 204 is undefined", async () => {
  const location = { directory: "/r", project: { id: "p", directory: "/r" } };
  const { client } = mockClient((req) => {
    if (req.url.pathname === "/api/v1/sessions/ses_1") return json(200, { data: { id: "ses_1" } });
    if (req.url.pathname === "/api/v1/sessions") return json(200, { location, data: { data: [], cursor: {} } });
    return new Response(null, { status: 204 });
  });
  assert.deepEqual(await client.session.get("ses_1"), { id: "ses_1" });
  assert.deepEqual(await client.session.list(), { location, data: { data: [], cursor: {} } });
  assert.equal(await client.session.interrupt("ses_1"), undefined);
});

test("a hung request times out with reason Timeout", async () => {
  const { client, calls } = mockClient(
    (req) =>
      new Promise<Response>((_resolve, reject) => req.signal?.addEventListener("abort", () => reject(req.signal?.reason))),
    { timeoutMs: 5, retry: { retries: 1, delayMs: 0 } },
  );
  const err = await client.session.create().catch((e: unknown) => e);
  assert.ok(err instanceof CyberClientError);
  assert.equal(err.reason, "Timeout");
  assert.equal(calls.length, 2);
});


test("BudgetExceededError retains its scope message and typed guard", async () => {
  const { client } = mockClient(() => json(409, { _tag: "BudgetExceededError", message: "scope ses_parent: max_tokens" }));
  const err = await client.session.compact("ses_parent").catch((e: unknown) => e);
  assert.ok(isBudgetExceededError(err));
  assert.ok(!isSessionBusyError(err));
  assert.equal(err.status, 409);
  assert.match(err.body.message, /ses_parent/);
});
