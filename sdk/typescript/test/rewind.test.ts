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
