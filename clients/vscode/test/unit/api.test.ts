import { strict as assert } from "node:assert";
import { test } from "node:test";
import { classify, confirmIdentity, createClient, describe, preflight, routeTemplate, SCOPE_MESSAGE } from "../../src/extension/api.js";
import { CyberApiError } from "@cyber-code/sdk";

const token = `cyi_${"B".repeat(43)}`;

interface Seen {
  url: string;
  authorization: string | null;
}

function fakeServer(routes: Record<string, (seen: Seen) => Response>): { fetch: typeof fetch; seen: Seen[] } {
  const seen: Seen[] = [];
  const fetcher = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const headers = new Headers(init?.headers);
    const entry = { url, authorization: headers.get("authorization") };
    seen.push(entry);
    const path = new URL(url).pathname;
    const route = routes[path];
    return route ? route(entry) : new Response(JSON.stringify({ _tag: "NotFound", message: "" }), { status: 404 });
  }) as typeof fetch;
  return { fetch: fetcher, seen };
}

const json = (value: unknown, status = 200, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json", ...headers } });

test("the token is sent only as a Bearer header, never in a URL or a log line", async () => {
  const server = fakeServer({
    "/api/v1/service": () => json({ id: "srv_1", version: "1", build: "b", pid: 1 }, 200, { "x-request-id": "req_1" }),
    "/api/v1/sessions/ses_ABC/messages": () => json({ data: [], cursor: {} }),
  });
  const logged: string[] = [];
  const client = createClient({ baseUrl: "http://localhost", fetch: server.fetch, token, directory: "/repo", log: (line) => logged.push(line) });
  assert.equal(await confirmIdentity(client, { id: "srv_1" }), true);
  await client.request("v1.message.list", { path: { sessionID: "ses_ABC" }, query: { limit: 5 } });
  for (const seen of server.seen) {
    assert.equal(seen.authorization, `Bearer ${token}`);
    assert.ok(!seen.url.includes(token) && !seen.url.includes("auth_token"), seen.url);
  }
  assert.deepEqual(logged, ["GET /api/v1/service 200 req_1", "GET /api/v1/sessions/{id}/messages 200 -"]);
  assert.ok(!logged.join("\n").includes(token) && !logged.join("\n").toLowerCase().includes("authorization"));
});

test("a mismatched server identity is detected after the first authenticated call", async () => {
  const server = fakeServer({ "/api/v1/service": () => json({ id: "srv_other" }) });
  const client = createClient({ baseUrl: "http://localhost", fetch: server.fetch, token, directory: "/repo", log: () => {} });
  assert.equal(await confirmIdentity(client, { id: "srv_1" }), false);
});

test("TCP preflight sends no credential and refuses another build", async () => {
  const healthy = { apiVersion: "v1", version: "0.2.0", build: "abc", features: ["session", "ide"] };
  const registration = { id: "srv_1", version: "0.2.0", build: "abc", pid: process.pid, url: "http://127.0.0.1:1" };
  const server = fakeServer({ "/api/v1/health": () => json(healthy) });
  assert.deepEqual(await preflight(server.fetch, "http://127.0.0.1:1", registration, true), { ok: true });
  const other = fakeServer({ "/api/v1/health": () => json({ ...healthy, build: "zzz" }) });
  const refused = await preflight(other.fetch, "http://127.0.0.1:1", registration, true);
  assert.equal(refused.ok, false);
  const dead = await preflight(server.fetch, "http://127.0.0.1:1", { ...registration, pid: 999_999_999 }, true);
  assert.equal(dead.ok, false);
  for (const seen of [...server.seen, ...other.seen]) assert.equal(seen.authorization, null);
});

test("an incompatible server: another API version or no ide group", async () => {
  const registration = { version: "0.1.0", build: "x" };
  for (const health of [{ apiVersion: "v2", features: ["ide"] }, { apiVersion: "v1", features: ["session"] }]) {
    const server = fakeServer({ "/api/v1/health": () => json(health) });
    const probe = await preflight(server.fetch, "http://localhost", registration, false);
    assert.equal(probe.ok, false);
  }
});

test("Out-of-scope action: ide_token_scope is shown as needing the CLI or dashboard", () => {
  const scope = new CyberApiError(403, { _tag: "ForbiddenError", message: "outside", reason: "ide_token_scope" } as never);
  assert.equal(classify(scope), "scope");
  assert.equal(describe(scope), SCOPE_MESSAGE);
  assert.equal(classify(new CyberApiError(401, { _tag: "UnauthorizedError", message: "" } as never)), "unauthorized");
});

test("route templates drop IDs and query strings", () => {
  assert.equal(routeTemplate(`http://localhost/api/v1/sessions/ses_01H/permissions/per_01J/reply?auth_token=${token}`), "/api/v1/sessions/{id}/permissions/{id}/reply");
});
