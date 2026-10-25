import assert from "node:assert/strict";
import { test } from "node:test";
import { ApiFault, HttpApi } from "../src/transport.js";
function api(fetcher: typeof fetch) { const value = new HttpApi("https://owner.fixture", () => {}, fetcher); value.password("fixture-only"); return value; }
test("private PNG reads send owner and Location headers with no caching or redirects", async () => {
 let init: RequestInit | undefined; const client = api(async (_url, options) => { init = options; return new Response(new Uint8Array([1, 2, 3]), { headers: { "content-type": "image/png" } }); });
 assert.deepEqual(await client.bytes("/sessions/ses_one/computer/frames/cuf_one", { directory: "/project" }), new Uint8Array([1, 2, 3]));
 assert.equal(new Headers(init?.headers).get("authorization"), `Basic ${btoa("cyber:fixture-only")}`); assert.equal(new Headers(init?.headers).get("x-cyber-directory"), "%2Fproject"); assert.equal(init?.cache, "no-store"); assert.equal(init?.redirect, "error");
});
test("private PNG reads reject other MIME types and oversized declared or streamed bodies", async () => {
 for (const response of [new Response("<html>bad</html>", { headers: { "content-type": "text/html" } }), new Response("bad", { headers: { "content-type": "image/png", "content-length": String(8 * 1024 * 1024 + 1) } }), new Response(new Uint8Array(8 * 1024 * 1024 + 1), { headers: { "content-type": "image/png" } })]) { await assert.rejects(api(async () => response).bytes("/private-frame"), ApiFault); }
});
test("logout discards a PNG that finishes reading after authentication was cleared", async () => {
 let source!: ReadableStreamDefaultController<Uint8Array>; const body = new ReadableStream<Uint8Array>({ start(controller) { source = controller; } }); const client = api(async () => new Response(body, { headers: { "content-type": "image/png" } })); const pending = client.bytes("/private-frame"); await Promise.resolve(); client.clear(); source.enqueue(new Uint8Array([1])); source.close(); await assert.rejects(pending, (error: unknown) => error instanceof ApiFault && error.kind === "auth");
});
