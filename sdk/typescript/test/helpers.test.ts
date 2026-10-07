import assert from "node:assert/strict";
import { test } from "node:test";
import { durable, frame, json, liveSse, mockClient, type Recorded } from "./support.js";

const HELLO = frame({ id: "live:0", type: "server.connected" });

test("prompt with wait subscribes first and resolves on the next idle", async () => {
  let stream: ReturnType<typeof liveSse> | undefined;
  const { client, calls } = mockClient((req: Recorded) => {
    if (req.url.pathname === "/api/v1/event") {
      stream = liveSse(req.signal);
      stream.send(HELLO);
      // A stale idle from an earlier Drain must not end the wait.
      stream.send(frame({ id: "live:1", type: "session.idle", data: { session_id: "ses_1" } }));
      return stream.response;
    }
    const send = stream!.send;
    setTimeout(() => {
      send(durable("ses_1", 10, "session.prompt.admitted.1"));
      send(durable("ses_1", 11, "session.step.started.1"));
      send(durable("ses_2", 12, "session.text.ended.1", { text: "other session" }));
      send(durable("ses_1", 12, "session.text.ended.1", { message_id: "m", text: "first" }));
      send(durable("ses_1", 13, "session.step.ended.1", { finish: "tool_calls", usage: { input: 3, output: 1, reasoning: 0, cache_read: 0, cache_write: 0 } }));
      send(durable("ses_1", 14, "session.text.ended.1", { message_id: "m2", text: "done" }));
      send(durable("ses_1", 15, "session.step.ended.1", { finish: "stop", usage: { input: 5, output: 2, reasoning: 1, cache_read: 4, cache_write: 0 } }));
      send(frame({ id: "live:9", type: "session.idle", data: { session_id: "ses_1" } }));
    }, 1);
    return json(202, { data: { session_id: "ses_1", message_id: "m", delivery: "steer", admitted_seq: 10, status: "pending" } });
  });
  const result = await client.sessions.prompt("ses_1", "fix the test", { wait: true });
  assert.equal(result.text, "done");
  assert.equal(result.stopReason, "stop");
  assert.deepEqual(result.usage, { input: 8, output: 3, reasoning: 1, cache_read: 4, cache_write: 0 });
  assert.equal(result.receipt.admitted_seq, 10);
  assert.deepEqual(calls.map((c) => `${c.method} ${c.url.pathname}${c.url.search}`), [
    "GET /api/v1/event?scope=all",
    "POST /api/v1/sessions/ses_1/prompt",
  ]);
  assert.deepEqual(JSON.parse(calls[1]?.body ?? ""), { parts: [{ type: "text", text: "fix the test" }], delivery: "steer" });
  assert.ok(calls[0]?.signal?.aborted, "the event stream is closed afterwards");
});

const permission = (id: string, session = "ses_1") => ({
  id,
  session_id: session,
  call_id: "c",
  message_id: "m",
  kind: "permission",
  action: "bash",
  resources: [id],
  always_patterns: [],
});

/** A mock server for onRequest: the event stream, pending lists, Location and Session lookups, and replies. */
function requestServer(options: { pending?: unknown[]; events?: (send: (chunk: string) => void, connection: number) => void; sessions?: Record<string, string> }) {
  const replies: Recorded[] = [];
  let connections = 0;
  const { client } = mockClient((req) => {
    const path = req.url.pathname;
    if (path === "/api/v1/event") {
      const stream = liveSse(req.signal);
      stream.send(HELLO);
      options.events?.(stream.send, connections++);
      return stream.response;
    }
    if (path.endsWith("/requests")) return json(200, { data: options.pending ?? [] });
    if (path === "/api/v1/location") return json(200, { data: { directory: "/repo/a", project: { id: "p", directory: "/repo/a" } } });
    if (path.startsWith("/api/v1/sessions/") && req.method === "GET") {
      const id = path.split("/")[4] ?? "";
      return json(200, { data: { id, directory: options.sessions?.[id] ?? "/repo/a" } });
    }
    replies.push(req);
    return new Response(null, { status: 204 });
  });
  return { client, replies };
}

async function until(condition: () => boolean): Promise<void> {
  const deadline = Date.now() + 2_000;
  while (!condition() && Date.now() < deadline) await new Promise((r) => setTimeout(r, 1));
  assert.ok(condition(), "expected request handler did not complete");
}

test("permissions.onRequest replies with the handler's decision", async () => {
  const { client, replies } = requestServer({ events: (send) => send(durable("ses_1", 5, "permission.asked.1", permission("per_1"))) });
  const seen: string[] = [];
  const stop = client.permissions.onRequest((request) => {
    seen.push(`${request.action}:${request.resources.join(",")}`);
    return "once";
  });
  await until(() => replies.length > 0);
  stop();
  assert.deepEqual(seen, ["bash:per_1"]);
  assert.equal(replies[0]?.url.pathname, "/api/v1/sessions/ses_1/permissions/per_1/reply");
  assert.deepEqual(JSON.parse(replies[0]?.body ?? ""), { reply: "once" });
});

test("onRequest handles requests pending at (re)connect once each, in the client's Location only", async () => {
  const { client, replies } = requestServer({
    // per_1 was asked while disconnected; per_2 belongs to another Location.
    pending: [permission("per_1"), permission("per_2", "ses_b")],
    sessions: { ses_b: "/repo/b" },
    events: (send, connection) => {
      // The asked event for per_1 also arrives live: it must not be answered twice.
      send(durable("ses_1", 5, "permission.asked.1", permission("per_1")));
      if (connection === 0) send(durable("ses_1", 6, "permission.asked.1", permission("per_3")));
    },
  });
  const seen: string[] = [];
  const stop = client.permissions.onRequest((request) => {
    seen.push(request.id);
    return { reply: "reject", message: "no" };
  });
  await until(() => replies.length >= 2);
  await new Promise((r) => setTimeout(r, 10));
  stop();
  assert.deepEqual(seen.sort(), ["per_1", "per_3"]);
  assert.deepEqual(replies.map((r) => r.url.pathname).sort(), [
    "/api/v1/sessions/ses_1/permissions/per_1/reply",
    "/api/v1/sessions/ses_1/permissions/per_3/reply",
  ]);
});

test("questions.onRequest dismisses when the handler returns undefined", async () => {
  const asked = { id: "que_1", session_id: "ses_1", call_id: "c", message_id: "m", kind: "question", questions: [] };
  const { client, replies } = requestServer({ events: (send) => send(durable("ses_1", 6, "question.asked.1", asked)) });
  const stop = client.questions.onRequest(() => undefined);
  await until(() => replies.length > 0);
  stop();
  assert.equal(replies[0]?.url.pathname, "/api/v1/sessions/ses_1/questions/que_1/reply");
  assert.deepEqual(JSON.parse(replies[0]?.body ?? ""), {});
});

test("onRequest recovers a child in another Location through trusted ancestor routes", async () => {
  const child = {
    ...permission("per_child", "ses_child"),
    origin: { title: "Review changes (@general)", agent: "general", directory: "/repo/worktree" },
    routed_to: [{ session_id: "ses_parent", directory: "/repo/a" }],
  };
  const other = { ...permission("per_other", "ses_other"), metadata: { routed_to: [{ session_id: "ses_parent", directory: "/repo/a" }] }, routed_to: [{ session_id: "ses_elsewhere", directory: "/repo/b" }] };
  const { client, replies } = requestServer({
    pending: [child, other], sessions: { ses_child: "/repo/worktree", ses_other: "/repo/b" },
  });
  const seen: string[] = [];
  const stop = client.permissions.onRequest((request) => { seen.push(request.id); return "once"; });
  try { await until(() => replies.length > 0); } finally { stop(); }
  assert.deepEqual(seen, ["per_child"]);
  assert.equal(replies[0]?.url.pathname, "/api/v1/sessions/ses_child/permissions/per_child/reply");
});

test("routed live and pending child requests are handled once with their origin name", async () => {
  const child = {
    ...permission("per_child", "ses_child"),
    origin: { title: "Review changes (@general)", agent: "general", directory: "/worktree" },
    routed_to: [{ session_id: "ses_parent", directory: "/repo/a" }],
  };
  const { client, replies } = requestServer({
    pending: [child], sessions: { ses_child: "/worktree" },
    events: (send) => {
      send(frame({ id: "live:1", type: "permission.asked.1", data: child }));
      send(frame({ id: "live:2", type: "permission.asked.1", data: child }));
    },
  });
  const names: string[] = [];
  const stop = client.permissions.onRequest((request) => { names.push(request.origin?.title ?? ""); return "once"; });
  try { await until(() => replies.length > 0); } finally { stop(); }
  assert.deepEqual(names, ["Review changes (@general)"]);
  assert.equal(replies.length, 1);
});

test("questions recover across child worktree Locations with their original owner", async () => {
  const child = {
    id: "que_child", session_id: "ses_child", call_id: "c", message_id: "m", kind: "question", questions: [],
    origin: { title: "Inspect storage (@explore)", agent: "explore", directory: "/worktree" },
    routed_to: [{ session_id: "ses_parent", directory: "/repo/a" }],
  };
  const { client, replies } = requestServer({ pending: [child], sessions: { ses_child: "/worktree" } });
  const names: string[] = [];
  const stop = client.questions.onRequest((request) => { names.push(request.origin?.title ?? ""); return []; });
  try { await until(() => replies.length > 0); } finally { stop(); }
  assert.deepEqual(names, ["Inspect storage (@explore)"]);
  assert.equal(replies[0]?.url.pathname, "/api/v1/sessions/ses_child/questions/que_child/reply");
});
