import assert from "node:assert/strict";
import { test } from "node:test";
import { AsyncQueue } from "../src/queue.js";
import type { RpcIo } from "../src/rpc.js";
import { ReconnectingChannel, ToolRegistry, type AppTool } from "../src/tools.js";

interface Frame {
  id?: number | string;
  method?: string;
  params?: Record<string, unknown>;
  result?: unknown;
  error?: { code: number; message: string };
}

/** One fake server connection: answers register/unregister and records every frame from the client. */
let nextRegistration = 0;
class FakePeer {
  readonly registrations = new Map<string, string>();
  readonly received: Frame[] = [];
  readonly lines = new AsyncQueue<string>();
  private readonly waiters = new Map<string, (frame: Frame) => void>();

  io(): RpcIo {
    return {
      send: (line) => this.receive(JSON.parse(line) as Frame),
      lines: this.lines,
      close: () => this.lines.end(),
    };
  }

  /** Send a server→client `tool.execute` request and resolve with the client's reply. */
  execute(id: string, params: Record<string, unknown>): Promise<Frame> {
    if (!("registration_id" in params)) params = { ...params, registration_id: this.registrations.get(String(params.name)) };
    const reply = new Promise<Frame>((resolve) => this.waiters.set(id, resolve));
    this.lines.push(JSON.stringify({ jsonrpc: "2.0", id, method: "tool.execute", params }));
    return reply;
  }

  registered(): string[] {
    return this.received.filter((f) => f.method === "v1.tool.register").map((f) => String(f.params?.name));
  }

  private receive(frame: Frame): void {
    this.received.push(frame);
    if (frame.method === undefined) return this.waiters.get(String(frame.id))?.(frame);
    const registration = `reg_${++nextRegistration}`;
    if (frame.method === "v1.tool.register") this.registrations.set(String(frame.params?.name), registration);
    const reply =
      frame.params?.name === "bash"
        ? { error: { code: -32602, message: "bash is a built-in tool" } }
        : { result: frame.method === "v1.tool.register" ? { registered: frame.params?.name, registration_id: registration } : { unregistered: true } };
    queueMicrotask(() => this.lines.push(JSON.stringify({ jsonrpc: "2.0", id: frame.id, ...reply })));
  }
}

function setup() {
  const peers: FakePeer[] = [];
  let handle: ReconnectingChannel | undefined;
  const registry = new ToolRegistry((attach) => {
    handle = new ReconnectingChannel(
      async () => {
        const peer = new FakePeer();
        peers.push(peer);
        return peer.io();
      },
      { initialMs: 1, maxMs: 4 },
      attach,
    );
    return handle;
  });
  return { registry, peers, closeHandle: () => handle?.close() };
}

const lookup: AppTool<{ id: string }> = {
  name: "lookup_ticket",
  description: "Look up a ticket",
  input: { type: "object", properties: { id: { type: "string" } }, required: ["id"] },
  execute: (input, context) => `ticket ${input.id} for ${context.sessionID}/${context.callID}`,
};

test("register sends the tool spec and execute replies with the output", async () => {
  const { registry, peers, closeHandle } = setup();
  await registry.register(lookup);
  const peer = peers[0]!;
  assert.deepEqual(peer.received[0]?.params, { name: lookup.name, description: lookup.description, input: lookup.input });
  const reply = await peer.execute("rpc_1", { name: "lookup_ticket", input: { id: "T-1" }, session_id: "ses_1", call_id: "call_1" });
  assert.deepEqual(reply, { jsonrpc: "2.0", id: "rpc_1", result: "ticket T-1 for ses_1/call_1" });
  closeHandle();
});

test("a throwing tool replies with a JSON-RPC error; non-string outputs pass through", async () => {
  const { registry, peers, closeHandle } = setup();
  await registry.register({ ...lookup, name: "boom", execute: () => Promise.reject(new Error("ticket system down")) });
  await registry.register({ ...lookup, name: "structured", execute: () => ({ ok: true }) });
  const peer = peers[0]!;
  const failed = await peer.execute("rpc_2", { name: "boom", input: {} });
  assert.deepEqual(failed.error, { code: -32000, message: "ticket system down" });
  const structured = await peer.execute("rpc_3", { name: "structured", input: {} });
  assert.deepEqual(structured.result, { ok: true });
  const unknown = await peer.execute("rpc_4", { name: "nope", input: {} });
  assert.match(unknown.error?.message ?? "", /not registered/);
  closeHandle();
});

test("tools are re-registered after the channel reconnects", async () => {
  const { registry, peers, closeHandle } = setup();
  await registry.register(lookup);
  await registry.register({ ...lookup, name: "second" });
  peers[0]!.lines.end(); // The server went away.
  while (peers.length < 2 || peers[1]!.registered().length < 2) await new Promise((r) => setTimeout(r, 1));
  assert.deepEqual(peers[1]!.registered(), ["lookup_ticket", "second"]);
  const reply = await peers[1]!.execute("rpc_9", { name: "second", input: { id: "T-2" }, session_id: "s", call_id: "c" });
  assert.equal(reply.result, "ticket T-2 for s/c");
  closeHandle();
});

test("unregister removes the tool; the last one closes the channel; names are validated", async () => {
  const { registry, peers } = setup();
  const removeFirst = await registry.register(lookup);
  const removeSecond = await registry.register({ ...lookup, name: "second" });
  await removeFirst();
  assert.deepEqual(peers[0]!.received.at(-1)?.method, "v1.tool.unregister");
  assert.deepEqual(peers[0]!.received.at(-1)?.params, { name: "lookup_ticket" });
  await removeSecond();
  await new Promise((r) => setTimeout(r, 10));
  assert.equal(peers.length, 1, "no reconnect after the channel was closed on purpose");
  await assert.rejects(registry.register({ ...lookup, name: "9bad" }), /invalid tool name/);
  await assert.rejects(registry.register(lookup).then(() => registry.register(lookup)), /already registered/);
  await assert.rejects(registry.register({ ...lookup, name: "a".repeat(65) }), /invalid tool name/);
});

test("a refused registration rejects and closes the idle channel", async () => {
  const { registry, peers } = setup();
  await assert.rejects(registry.register({ ...lookup, name: "bash" }), /bash is a built-in tool/);
  let closed = false;
  void (async () => {
    for await (const _ of peers[0]!.lines);
    closed = true;
  })();
  await new Promise((r) => setTimeout(r, 10));
  assert.ok(closed, "the channel is closed so the process can exit");
  assert.equal(peers.length, 1, "and it is not redialed");
});

test("old and missing registration identities cannot reach a replacement executor", async () => {
  const { registry, peers, closeHandle } = setup();
  let executions = 0;
  const remove = await registry.register({ ...lookup, execute: () => { executions++; return "old"; } });
  await registry.register({ ...lookup, name: "keep_channel" });
  const peer = peers[0]!;
  const old = peer.registrations.get(lookup.name);
  await remove();
  await registry.register({ ...lookup, execute: () => { executions++; return "replacement"; } });
  assert.notEqual(peer.registrations.get(lookup.name), old);
  const stale = await peer.execute("old_registration", { name: lookup.name, registration_id: old });
  assert.equal(stale.error?.message, `Stale tool call: ${lookup.name}`);
  const missing = await peer.execute("missing_registration", { name: lookup.name, registration_id: undefined });
  assert.equal(missing.error?.message, `Stale tool call: ${lookup.name}`);
  assert.equal(executions, 0);
  const fresh = await peer.execute("fresh_registration", { name: lookup.name });
  assert.equal(fresh.result, "replacement");
  assert.equal(executions, 1);
  closeHandle();
});
