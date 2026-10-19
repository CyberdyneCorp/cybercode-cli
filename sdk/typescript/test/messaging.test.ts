// `client.messaging.history()`: follows `cursor.next` lazily with the same filter, and the
// generated messaging operations stay on the same group.
import assert from "node:assert/strict";
import { test } from "node:test";
import { json, mockClient } from "./support.js";

const record = (id: string) => ({ message_id: id, session_id: "ses_b", direction: "in" });

test("history yields every record across pages with the same filter", async () => {
  const { client, calls } = mockClient((req) =>
    req.url.searchParams.get("cursor") === "c1"
      ? json(200, { data: [record("msgx_2")], cursor: {} })
      : json(200, { data: [record("msgx_1")], cursor: { next: "c1" } }),
  );
  const ids: string[] = [];
  for await (const r of client.messaging.history("ses_b", { direction: "in", order: "asc", limit: 1 })) ids.push(r.message_id);
  assert.deepEqual(ids, ["msgx_1", "msgx_2"]);
  assert.equal(calls.length, 2);
  assert.equal(calls[0]!.url.pathname, "/api/v1/sessions/ses_b/messaging");
  assert.deepEqual(Object.fromEntries(calls[0]!.url.searchParams), { direction: "in", order: "asc", limit: "1" });
  assert.deepEqual(Object.fromEntries(calls[1]!.url.searchParams), { direction: "in", order: "asc", limit: "1", cursor: "c1" });
  assert.equal(typeof client.messaging.send, "function");
  assert.equal(typeof client.messaging.sessions, "function");
  assert.equal(typeof client.messaging.watch, "function");
});

test("history reads nothing until iterated", async () => {
  const { client, calls } = mockClient(() => json(200, { data: [], cursor: {} }));
  const pages = client.messaging.history("ses_b");
  assert.equal(calls.length, 0);
  assert.deepEqual(await pages.next(), { value: undefined, done: true });
  assert.equal(calls.length, 1);
});
