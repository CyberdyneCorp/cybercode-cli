import { strict as assert } from "node:assert";
import { test } from "node:test";
import { PendingTable, type PendingRequest } from "../../src/extension/approvals.js";

const ask = (id: string, session = "ses_A", extra: Partial<PendingRequest> = {}): PendingRequest => ({
  id, session_id: session, kind: "permission", action: "edit", resources: ["src/app.ts"], always_patterns: ["src/**"], metadata: { diff: "--- a/x\n+++ b/x\n" }, ...extra,
});

test("Requests from a disconnect appear as cards, child requests included", () => {
  const table = new PendingTable();
  table.bind("ses_A");
  table.update([ask("per_1"), ask("per_2", "ses_child", { root_session_id: "ses_A" }), ask("per_3", "ses_other")]);
  const cards = table.cards();
  assert.deepEqual(cards.map((card) => card.requestId), ["per_1", "per_2"]);
  assert.equal(cards[1]?.origin, "ses_child");
  assert.equal(cards[0]?.hasDiff, true);
});

test("Forged approval refused: only a pending request of its own Session", () => {
  const table = new PendingTable();
  table.bind("ses_A");
  table.update([ask("per_1")]);
  assert.equal(table.begin("ses_A", "per_123", { reply: "once" }), undefined);
  assert.equal(table.begin("ses_B", "per_1", { reply: "once" }), undefined);
  assert.ok(table.begin("ses_A", "per_1", { reply: "once" }));
});

test("Stale card cannot approve a later request", () => {
  const table = new PendingTable();
  table.bind("ses_A");
  table.update([ask("per_1")]);
  assert.equal(table.answered("per_1"), true);
  table.update([ask("per_2")]);
  assert.equal(table.begin("ses_A", "per_1", { reply: "once" }), undefined);
  assert.equal(table.cards().find((card) => card.requestId === "per_1")?.state, "stale");
  assert.equal(table.cards().find((card) => card.requestId === "per_2")?.state, "pending");
});

test("Uncertain reply retried with the same request, reply and idempotency key", () => {
  const table = new PendingTable();
  table.bind("ses_A");
  table.update([ask("per_1")]);
  const first = table.begin("ses_A", "per_1", { reply: "once" });
  assert.ok(first);
  assert.equal(table.begin("ses_A", "per_1", { reply: "once" }), undefined, "no second reply while one is in flight");
  table.unconfirmed("per_1");
  assert.equal(table.cards()[0]?.state, "unconfirmed");
  assert.equal(table.begin("ses_A", "per_1", { reply: "reject" }), undefined, "an unconfirmed decision cannot change");
  const retry = table.begin("ses_A", "per_1", { reply: "once" });
  assert.equal(retry?.key, first.key);
  table.acknowledge("per_1");
  assert.equal(table.cards().length, 0);
});

test("a refused reply may be changed, and rebinding drops other Sessions' cards", () => {
  const table = new PendingTable();
  table.bind("ses_A");
  table.update([ask("per_1")]);
  const first = table.begin("ses_A", "per_1", { reply: "once" });
  table.refused("per_1");
  const second = table.begin("ses_A", "per_1", { reply: "reject", message: "no" });
  assert.notEqual(second?.key, first?.key);
  table.bind("ses_B");
  assert.equal(table.cards().length, 0);
});

test("taint notices are shown verbatim and questions keep their options", () => {
  const table = new PendingTable();
  table.bind("ses_A");
  table.update([
    ask("per_1", "ses_A", { metadata: { command: "curl x", taint: "untrusted web content" } }),
    { id: "que_1", session_id: "ses_A", kind: "question", questions: [{ header: "Plan", question: "Approve the plan?", options: [{ label: "Approve and build" }, { label: "Approve with accept-edits" }, { label: "Request changes" }] }] },
  ]);
  const [permission, question] = table.cards();
  assert.equal(permission?.taint, "untrusted web content");
  assert.equal(permission?.detail, "curl x");
  assert.deepEqual(question?.questions[0]?.options, ["Approve and build", "Approve with accept-edits", "Request changes"]);
});
