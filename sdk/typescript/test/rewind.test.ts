import assert from "node:assert/strict";
import { test } from "node:test";
import { json, mockClient } from "./support.js";

test("reviewed rewind operations preserve the selected scope and fingerprint", async () => {
  const selection = { message_id: "msg_boundary", target: "both" as const, expected_directory: "/work/project" };
  const fingerprint = "a".repeat(64);
  const { client, calls } = mockClient(req => json(200, { data: req.url.pathname.endsWith("/preview")
    ? { ...selection, session_id: "ses_one", directory: selection.expected_directory, sequence: 19,
        active_tip: "msg_tip", code: null, effects: [], warning: "External effects remain", confirmation: fingerprint }
    : { message_id: selection.message_id, target: selection.target, baseline: null, applied: null, diff: "" } }));
  const preview = await client.request("v1.session.rewindPreview", { path: { sessionID: "ses_one" }, body: selection });
  await client.request("v1.session.rewindConfirm", { path: { sessionID: "ses_one" }, body: { ...selection, confirmation: preview.confirmation } });
  assert.equal(calls.length, 2);
  assert.equal(calls[0]!.url.pathname, "/api/v1/sessions/ses_one/rewind/preview");
  assert.equal(calls[1]!.url.pathname, "/api/v1/sessions/ses_one/rewind/confirm");
  assert.deepEqual(JSON.parse(calls[1]!.body!), { ...selection, confirmation: fingerprint });
});

test("snapshot recovery review preserves owner scope and honest acknowledgement limits", async () => {
  const review = { journal_id: "restore_one", action: "keep", origin: null, filesystem_complete: true,
    acknowledged: false, confirmation: null, paths: [], exclusions: null, recorded_created_parents: null,
    conflicts: ["Legacy intent has no exact native worker origin"] };
  const { client, calls } = mockClient(() => json(200, { data: review }));
  const result = await client.session.snapshotRecoveryReview("ses_owner", { journal_id: "restore_one", action: "keep" });
  assert.deepEqual(result, review);
  assert.equal(result.acknowledged, false);
  assert.equal(result.confirmation, null);
  assert.equal(calls.length, 1);
  assert.equal(calls[0]!.method, "POST");
  assert.equal(calls[0]!.url.pathname, "/api/v1/sessions/ses_owner/rewind/recovery/review");
  assert.deepEqual(JSON.parse(calls[0]!.body!), { journal_id: "restore_one", action: "keep" });
});

test("snapshot recovery confirmation and lookup retain the exact request without implying journal acknowledgement", async () => {
  const request = { journal_id: "restore_one", action: "keep" as const, confirmation: "a".repeat(64), request_id: "request/with space" };
  const result = { request, original: {}, resolution: null, state: "unknown", receipt: null, journal_acknowledged: null };
  const { client, calls } = mockClient(() => json(200, { data: result }));
  assert.equal((await client.session.snapshotRecoveryConfirm("ses_owner", request)).state, "unknown");
  const lookup = await client.session.snapshotRecoveryLookup("ses_owner", request.request_id);
  assert.equal(lookup?.journal_acknowledged, null);
  assert.equal(calls.length, 2);
  assert.equal(calls[0]!.method, "POST");
  assert.equal(calls[0]!.url.pathname, "/api/v1/sessions/ses_owner/rewind/recovery/confirm");
  assert.deepEqual(JSON.parse(calls[0]!.body!), request);
  assert.equal(calls[1]!.method, "GET");
  assert.equal(calls[1]!.url.pathname, "/api/v1/sessions/ses_owner/rewind/recovery/requests/request%2Fwith%20space");
});

test("snapshot recovery acknowledgement uses the retained request without a replay body", async () => {
  const result = { state: "acknowledged", journal_acknowledged: true, receipt: { resolution_digest: "d".repeat(64) } };
  const { client, calls } = mockClient(() => json(200, { data: result }));
  const acknowledged = await client.session.snapshotRecoveryAcknowledge("ses_owner", "request/with space");
  assert.equal(acknowledged.journal_acknowledged, true);
  assert.equal(acknowledged.receipt?.resolution_digest, "d".repeat(64));
  assert.equal(calls.length, 1);
  assert.equal(calls[0]!.method, "POST");
  assert.equal(calls[0]!.url.pathname, "/api/v1/sessions/ses_owner/rewind/recovery/requests/request%2Fwith%20space/acknowledge");
  assert.ok(!calls[0]!.body);
});

test("ordinary restore lookup and acknowledgement preserve immutable paired receipt identity", async () => {
  const receipt = { origin: { activity_id: "op_ordinary" }, intent_digest: "d".repeat(64),
    session_before: 9, session_after: 11, events_digest: "e".repeat(64) };
  const { client, calls } = mockClient(() => json(200, { data: receipt }));
  assert.deepEqual(await client.session.snapshotRestoreLookup("ses_owner", "op_ordinary"), receipt);
  assert.deepEqual(await client.session.snapshotRestoreAcknowledge("ses_owner", "op_ordinary"), receipt);
  assert.equal(calls[0]!.method, "GET");
  assert.equal(calls[0]!.url.pathname, "/api/v1/sessions/ses_owner/rewind/restores/op_ordinary");
  assert.equal(calls[1]!.method, "POST");
  assert.equal(calls[1]!.url.pathname, "/api/v1/sessions/ses_owner/rewind/restores/op_ordinary/acknowledge");
  assert.ok(!calls[1]!.body);
});
