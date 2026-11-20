import { strict as assert } from "node:assert";
import { test } from "node:test";
import { hostMessage, MAX_MESSAGE_BYTES, webviewMessage } from "../../src/shared/protocol.js";

const reply = { type: "reply", sessionId: "ses_01ABC", requestId: "per_01XYZ", reply: "once" };

test("well-formed webview messages pass", () => {
  assert.deepEqual(webviewMessage(reply), reply);
  assert.ok(webviewMessage({ type: "prompt", text: "hi", includeContext: true }));
  assert.ok(webviewMessage({ type: "answer", sessionId: "ses_1", requestId: "que_1", answers: [["Approve and build"]] }));
  assert.ok(webviewMessage({ type: "answer", sessionId: "ses_1", requestId: "que_1" }));
});

test("unknown types, extra fields, bad IDs and wrong shapes are dropped", () => {
  for (const value of [
    { type: "eval", code: "1" },
    { type: "toString" },
    { ...reply, extra: 1 },
    { ...reply, requestId: "per_1/../x" },
    { ...reply, requestId: "evt_1" },
    { ...reply, sessionId: "ses_" },
    { ...reply, reply: "bypass" },
    { type: "setMode", mode: "bypass" },
    { type: "setMode", mode: "dont-ask" },
    { type: "action", action: "deleteEverything" },
    { type: "prompt", text: 1, includeContext: true },
    null,
    "reply",
    [reply],
  ]) {
    assert.equal(webviewMessage(value), undefined, JSON.stringify(value));
  }
});

test("payloads over 1 MiB are dropped on both sides", () => {
  const text = "x".repeat(MAX_MESSAGE_BYTES);
  assert.equal(webviewMessage({ type: "prompt", text, includeContext: false }), undefined);
  assert.equal(hostMessage({ type: "insert", text }), undefined);
  assert.ok(hostMessage({ type: "insert", text: "@src/app.ts#L1" }));
});
