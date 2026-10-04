import assert from "node:assert/strict";
import { test } from "node:test";
import { durable, frame, json, mockClient, sse } from "./support.js";

test("a session stream resumes from the last seen seq after a disconnect", async () => {
  const { client, calls } = mockClient((req, i) => {
    if (i === 0) return sse([durable("ses_1", 1, "session.created.1"), durable("ses_1", 2, "session.prompt.admitted.1").slice(0, 30)]);
    if (i === 1) return sse([durable("ses_1", 2, "session.prompt.admitted.1"), durable("ses_1", 3, "session.step.started.1")], { fail: true });
    if (i === 2) return new Response("unavailable", { status: 503 });
    // Replays seq 3 again (as a server might); the client drops it.
    return sse([durable("ses_1", 3, "session.step.started.1"), durable("ses_1", 4, "session.text.ended.1", { text: "hi" })]);
  });
  const seen: number[] = [];
  for await (const event of client.sessions.events("ses_1", { after: 0 })) {
    seen.push(event.durable?.seq ?? -1);
    if (seen.length === 4) break;
  }
  assert.deepEqual(seen, [1, 2, 3, 4]);
  assert.deepEqual(
    calls.map((c) => c.url.searchParams.get("after")),
    ["0", "1", "3", "3"],
  );
  assert.equal(calls[0]?.headers.get("accept"), "text/event-stream");
  assert.ok(calls.at(-1)?.signal?.aborted, "break aborts the connection");
});

test("a client error ends the stream with that error", async () => {
  const { client, calls } = mockClient(() => json(404, { _tag: "SessionNotFoundError", message: "gone" }));
  await assert.rejects(async () => {
    for await (const _ of client.sessions.events("ses_x")) assert.fail("no events expected");
  }, /SessionNotFoundError/);
  assert.equal(calls.length, 1);
  assert.equal(calls[0]?.url.searchParams.get("after"), null);
});

test("the instance stream reconnects without a cursor", async () => {
  const hello = frame({ id: "live:0", type: "server.connected" });
  const { client, calls } = mockClient(() => sse([hello], { fail: true }));
  const types: string[] = [];
  for await (const event of client.events.subscribe()) {
    types.push(event.type);
    if (types.length === 3) break;
  }
  assert.deepEqual(types, ["server.connected", "server.connected", "server.connected"]);
  assert.equal(calls.length, 3);
  assert.ok(calls.every((c) => c.url.pathname === "/api/v1/event" && !c.url.searchParams.has("after")));
});
