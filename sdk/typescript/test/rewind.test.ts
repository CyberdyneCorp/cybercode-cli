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
