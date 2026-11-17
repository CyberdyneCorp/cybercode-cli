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

test("input-only attachment methods bind prompt attendance without using browser leases", async () => {
  const { client, calls } = mockClient((request) => {
    if (request.method === "DELETE") return new Response(null, { status: 204 });
    return json(200, { data: { client_id: "icl_input", session_id: "ses_shared", expires_at: 100 } });
  });
  const lease = await client.session.inputClientAttach("ses_shared");
  await client.session.prompt("ses_shared", { parts: [{ type: "text", text: "run" }], input_client: { client_id: lease.client_id } });
  await client.session.inputClientRenew("ses_shared", lease.client_id);
  await client.session.inputClientRelease("ses_shared", lease.client_id);
  assert.deepEqual(calls.map((call) => call.url.pathname), [
    "/api/v1/sessions/ses_shared/input-clients", "/api/v1/sessions/ses_shared/prompt",
    "/api/v1/sessions/ses_shared/input-clients/icl_input", "/api/v1/sessions/ses_shared/input-clients/icl_input",
  ]);
  assert.deepEqual(JSON.parse(calls[1]?.body ?? "null").input_client, { client_id: "icl_input" });
});
