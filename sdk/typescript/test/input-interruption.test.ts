import assert from "node:assert/strict";
import { test } from "node:test";
import { CyberApiError } from "../src/index.js";
import { json, mockClient } from "./support.js";

const body = { request_id: "op_owned", input_message_ids: ["msg_owned"] };
const receipt = {
  ...body, generation: "op_runtime", status: "requested", session_seq: 12,
  drain_selected: true, refused: 0, jobs_cancelled: 0, problems: [],
};

test("input interruption retries preserve the explicit scope and lookup observes the same request", async () => {
  const { client, calls } = mockClient((request, index) => {
    if (index === 0) throw new TypeError("lost interruption response");
    return json(200, { data: { ...receipt, status: request.method === "GET" ? "acknowledged" : "requested" } });
  });
  const accepted = await client.session.interruptInputs("ses_shared", body);
  assert.equal(accepted.request_id, body.request_id);
  const settled = await client.session.inputInterruption("ses_shared", body.request_id);
  assert.equal(settled.status, "acknowledged");
  assert.equal(settled.session_seq, 12);
  assert.deepEqual(calls.map((call) => call.method), ["POST", "POST", "GET"]);
  assert.deepEqual(calls.slice(0, 2).map((call) => JSON.parse(call.body ?? "null")), [body, body]);
  assert.equal(calls[0]?.headers.get("idempotency-key"), calls[1]?.headers.get("idempotency-key"));
  assert.equal(calls[2]?.url.pathname, "/api/v1/sessions/ses_shared/interrupt-inputs/op_owned");
  assert.equal(calls[2]?.body, undefined);
});

test("a scoped interruption conflict cannot fall back to Session-wide interruption", async () => {
  const { client, calls } = mockClient(() => json(409, { _tag: "ConflictError", message: "different cohort" }));
  const error = await client.session.interruptInputs("ses_shared", body).catch((error: unknown) => error);
  assert.ok(error instanceof CyberApiError);
  assert.equal(calls.length, 1);
  assert.equal(calls[0]?.url.pathname, "/api/v1/sessions/ses_shared/interrupt-inputs");
});
