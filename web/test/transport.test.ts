import assert from "node:assert/strict";
import { test } from "node:test";
import { ApiFault, HttpApi } from "../src/transport.js";

const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });

test("owner authentication is explicit, origin-bound and never ambient or URL encoded", async () => {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const api = new HttpApi("https://workspace.example", () => {}, async (input, init) => { calls.push({ url: String(input), init: init! }); return json({ data: [] }); });
  api.password("fixture-only-密碼");
  await api.request("/sessions?limit=50", { directory: "/private/project" });
  const { url, init } = calls[0]!;
  assert.equal(url, "https://workspace.example/api/v1/sessions?limit=50");
  assert.equal(init.credentials, "omit"); assert.equal(init.redirect, "error"); assert.equal(init.cache, "no-store");
  assert.equal(new Headers(init.headers).get("authorization"), `Basic ${Buffer.from("cyber:fixture-only-密碼").toString("base64")}`);
  assert.equal(new Headers(init.headers).get("x-cyber-client"), "web");
  assert.equal(new Headers(init.headers).get("x-cyber-directory"), encodeURIComponent("/private/project"));
  assert.ok(!url.includes("fixture-only"));
  for (const path of ["//foreign.test", "/../../foreign", "/event#token"]) await assert.rejects(api.request(path), ApiFault);
  assert.equal(calls.length, 1);
});

test("native default fetch keeps its browser receiver and auth failures erase usable credentials", async () => {
  const original = globalThis.fetch;
  let unauthorized = 0;
  let calls = 0;
  globalThis.fetch = async function (this: unknown) { assert.equal(this, globalThis); calls++; return json({}, 401); };
  try {
    const api = new HttpApi("https://workspace.example", () => unauthorized++);
    api.password("fixture-only");
    await assert.rejects(api.request("/location"), (error) => error instanceof ApiFault && error.kind === "auth");
    await assert.rejects(api.request("/location"), ApiFault);
    assert.equal(calls, 1); assert.equal(unauthorized, 1);
  } finally { globalThis.fetch = original; }
});

test("a late unauthorized response cannot erase credentials from a newer login", async () => {
  let resolve!: (response: Response) => void;
  let calls = 0;
  const headers: string[] = [];
  let unauthorized = 0;
  const api = new HttpApi("https://workspace.example", () => unauthorized++, async (_input, init) => {
    headers.push(new Headers(init!.headers).get("authorization")!);
    if (++calls === 1) return new Promise<Response>((done) => { resolve = done; });
    return json({ data: [] });
  });
  api.password("old-fixture"); const old = api.request("/location"); api.password("new-fixture");
  resolve(json({}, 401)); await assert.rejects(old, ApiFault);
  await api.request("/location");
  assert.equal(unauthorized, 0); assert.equal(headers[1], `Basic ${btoa("cyber:new-fixture")}`);
});

test("uncertain mutations retry the identical request identity and server errors reveal no raw secret", async () => {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const api = new HttpApi("https://workspace.example", () => {}, async (input, init) => {
    calls.push({ url: String(input), init: init! });
    if (calls.length === 1) throw new TypeError("fixture response lost");
    return new Response(null, { status: 204 });
  });
  api.password("fixture-only");
  const options = { method: "POST" as const, key: "same-answer-key", body: { reply: "once" } };
  await assert.rejects(api.request("/sessions/ses_child/permissions/req_one/reply", options), ApiFault);
  await api.request("/sessions/ses_child/permissions/req_one/reply", options);
  assert.equal(calls[0]!.url, calls[1]!.url); assert.equal(calls[0]!.init.body, calls[1]!.init.body);
  assert.equal(new Headers(calls[0]!.init.headers).get("idempotency-key"), new Headers(calls[1]!.init.headers).get("idempotency-key"));
  const refused = new HttpApi("https://workspace.example", () => {}, async () => json({ message: "do not echo fixture-only" }, 403));
  refused.password("fixture-only"); await assert.rejects(refused.request("/location"), (error) => error instanceof ApiFault && !error.message.includes("fixture-only"));
});

test("SSE carries owner headers and a durable cursor without EventSource or URL credentials", async () => {
  let recorded: RequestInit | undefined;
  let requested = "";
  const api = new HttpApi("https://workspace.example", () => {}, async (input, init) => {
    requested = String(input); recorded = init;
    const payload = 'id: ses_one:8\nevent: session.test.1\ndata: {"id":"ses_one:8","type":"session.test.1","data":{},"durable":{"aggregateID":"ses_one","seq":8,"version":1}}\n\n';
    return new Response(new ReadableStream({ start(controller) { const bytes = new TextEncoder().encode(payload); controller.enqueue(bytes.slice(0, 40)); controller.enqueue(bytes.slice(40)); } }), { headers: { "content-type": "text/event-stream" } });
  });
  api.password("fixture-only");
  let opened = false;
  const iterator = api.stream("/sessions/ses_one/events?after=7", { directory: "/project", signal: new AbortController().signal, opened: () => { opened = true; } });
  const event = await iterator.next();
  assert.equal(event.value?.durable?.seq, 8); assert.equal(opened, true);
  assert.equal(requested, "https://workspace.example/api/v1/sessions/ses_one/events?after=7");
  assert.equal(new Headers(recorded!.headers).get("accept"), "text/event-stream");
  assert.equal(new Headers(recorded!.headers).get("x-cyber-client"), "web");
  assert.equal(recorded!.credentials, "omit"); assert.equal(recorded!.redirect, "error");
  await iterator.return?.(); api.clear();
});
