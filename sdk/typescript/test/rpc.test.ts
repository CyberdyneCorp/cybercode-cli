import assert from "node:assert/strict";
import { test } from "node:test";
import { CyberApiError, CyberClient, CyberClientError, isSessionNotFoundError, RpcChannel, RpcTransport } from "../src/index.js";
import { AsyncQueue } from "../src/queue.js";
import { fixedChannel, ToolRegistry } from "../src/tools.js";

interface Request {
  id: number;
  method: string;
  params: Record<string, unknown>;
}

/** An in-memory JSON-RPC peer: `respond` answers each request; `emit` writes raw frames. */
function fakePeer(respond: (req: Request, peer: { emit(frame: unknown): void }) => unknown) {
  const lines = new AsyncQueue<string>();
  const requests: Request[] = [];
  const peer = { emit: (frame: unknown) => lines.push(JSON.stringify(frame)) };
  const channel = new RpcChannel({
    send(line) {
      const req = JSON.parse(line) as Request;
      requests.push(req);
      queueMicrotask(() => {
        const reply = respond(req, peer);
        if (reply !== undefined) peer.emit({ jsonrpc: "2.0", id: req.id, ...(reply as object) });
      });
    },
    lines,
  });
  const tools = new ToolRegistry(fixedChannel(channel));
  const client = new CyberClient(new RpcTransport(channel, "/repo/a", { initialMs: 1, maxMs: 4 }, tools));
  return { client, requests, lines, peer };
}

const event = (session: string, seq: number, type = "session.step.started.1") => ({
  id: `${session}:${seq}`,
  type,
  data: {},
  durable: { aggregateID: session, seq, version: 1 },
});

test("calls send operation IDs with path, query, body, directory and keys; results are unwrapped", async () => {
  const { client, requests } = fakePeer((req) => {
    if (req.method === "v1.session.get") return { result: { data: { id: "ses_1" } } };
    if (req.method === "v1.session.list") return { result: { location: {}, data: { data: [], cursor: {} } } };
    return { result: null };
  });
  assert.deepEqual(await client.session.get("ses_1"), { id: "ses_1" });
  assert.deepEqual(await client.at("/repo/b").session.list({ limit: 2, search: undefined }), { location: {}, data: { data: [], cursor: {} } });
  assert.equal(await client.session.update("ses_1", { title: "t" }, { idempotencyKey: "k1" }), undefined);
  assert.equal(await client.session.interrupt("ses_1"), undefined);
  assert.deepEqual(requests[0]?.params, { path: { sessionID: "ses_1" } });
  assert.deepEqual(requests[1]?.params, { query: { limit: 2 }, directory: "/repo/b" });
  assert.deepEqual(requests[2]?.params, { path: { sessionID: "ses_1" }, body: { title: "t" }, idempotencyKey: "k1" });
  assert.match(String(requests[3]?.params.idempotencyKey), /^[0-9a-f-]{36}$/);
});

test("errors map from error.data to CyberApiError with the HTTP status", async () => {
  const { client } = fakePeer((req) =>
    req.method === "v1.session.get"
      ? { error: { code: -32404, message: "SessionNotFoundError: x", data: { _tag: "SessionNotFoundError", message: "x" } } }
      : { error: { code: -32601, message: "unknown method", data: null } },
  );
  const err = await client.session.get("x").catch((e: unknown) => e);
  assert.ok(err instanceof CyberApiError && isSessionNotFoundError(err));
  assert.equal(err.status, 404);
  const other = await client.request("v1.health.get").catch((e: unknown) => e);
  assert.ok(other instanceof CyberClientError && other.reason === "UnexpectedStatus");
});

test("session events subscribe with after, buffer early notifications, drop duplicates and unsubscribe on break", async () => {
  const { client, requests } = fakePeer((req, peer) => {
    if (req.method !== "v1.session.events") return { result: { unsubscribed: true } };
    const subscription = "sub_1";
    // A notification may arrive before the subscription result.
    peer.emit({ jsonrpc: "2.0", method: "event", params: { subscription, event: event("ses_1", 5) } });
    queueMicrotask(() => {
      for (const seq of [5, 6, 7]) peer.emit({ jsonrpc: "2.0", method: "event", params: { subscription, event: event("ses_1", seq) } });
      peer.emit({ jsonrpc: "2.0", method: "event", params: { subscription: "sub_other", event: event("ses_2", 1) } });
    });
    return { result: { subscription } };
  });
  const seen: number[] = [];
  for await (const e of client.sessions.events("ses_1", { after: 4 })) {
    seen.push(e.durable?.seq ?? -1);
    if (seen.length === 3) break;
  }
  assert.deepEqual(seen, [5, 6, 7]);
  assert.equal(requests[0]?.method, "v1.session.events");
  assert.deepEqual(requests[0]?.params, { path: { sessionID: "ses_1" }, query: { after: 4 } });
  await new Promise((r) => setTimeout(r, 1));
  assert.equal(requests[1]?.method, "v1.rpc.unsubscribe");
  assert.deepEqual(requests[1]?.params, { subscription: "sub_1" });
});

test("when the peer exits, pending calls reject and streams end without reconnecting", async () => {
  const { client, lines, requests } = fakePeer((req) => (req.method === "v1.event.subscribe" ? { result: { subscription: "sub_1" } } : undefined));
  const pending = client.session.get("ses_1").catch((e: unknown) => e);
  const stream = (async () => {
    for await (const _ of client.events.subscribe()) assert.fail("no events expected");
  })().catch((e: unknown) => e);
  await new Promise((r) => setTimeout(r, 5));
  lines.end();
  const [callErr, streamErr] = await Promise.all([pending, stream]);
  for (const err of [callErr, streamErr]) {
    assert.ok(err instanceof CyberClientError && err.reason === "Transport" && err.permanent, String(err));
  }
  assert.equal(requests.filter((r) => r.method === "v1.event.subscribe").length, 1);
  await assert.rejects(client.health.get(), /closed the connection/);
});
