import assert from "node:assert/strict";
import { test } from "node:test";
import { Dashboard, type Lease } from "../src/model.js";
import { ApiFault, type Port, type RequestOptions, type StreamOptions } from "../src/transport.js";
import type { EventEnvelope, PendingRequest, Session } from "../../sdk/typescript/src/generated/types.js";

const session = (id: string, directory = "/project") => ({ id, directory, title: id, agent: "build", model: "fixture", mode: "default", status: "idle", seq: 0 }) as Session;
const permission = (id = "req_one", owner = "ses_child") => ({ id, session_id: owner, call_id: "call_one", message_id: "msg_one", kind: "permission", action: "bash", resources: ["echo fixture"], always_patterns: ["echo fixture"], metadata: { command: "echo fixture" }, routed_to: [{ session_id: "ses_one", directory: "/project" }] }) as PendingRequest;
const until = async (ready: () => boolean) => { for (let n = 0; n < 2000 && !ready(); n++) await new Promise((resolve) => setTimeout(resolve, 1)); assert.ok(ready(), "state became ready"); };
class Fake implements Port {
  now = 100_000; identity = "server-one"; requests: PendingRequest[] = [permission()]; calls: Array<{ path: string; options: RequestOptions }> = [];
  sessions = [session("ses_one"), session("ses_two", "/other")];
  failReply = false; commitReply = false; failAttach = false; failPrompt = false; cleared = 0;
  hook?: (path: string, options: RequestOptions) => Promise<unknown> | undefined;
  password(): void {} clear(): void { this.cleared++; }
  async request<T>(path: string, options: RequestOptions = {}): Promise<T> {
    this.calls.push({ path, options: structuredClone({ ...options, signal: undefined }) });
    const custom = this.hook?.(path, options); if (custom) return await custom as T;
    const result = (): unknown => {
      if (path === "/health") return { id: this.identity };
      if (path === "/location") return { data: { directory: options.directory ?? "/project", project: { id: "project", directory: options.directory ?? "/project" } } };
      if (path.startsWith("/sessions?")) return { data: { data: this.sessions, cursor: {} }, location: { directory: "/project" } };
      if (path === "/sessions" && options.method === "POST") return { data: this.sessions[0] };
      const id = path.split("/")[2];
      if (/^\/sessions\/[^/]+$/.test(path)) return { data: this.sessions.find((item) => item.id === id) };
      if (path.endsWith("/interactive-clients") && options.method === "POST") { if (this.failAttach) { this.failAttach = false; throw new ApiFault("network"); } return { data: { client_id: "icl_fixture", session_id: id, expires_at: this.now + 45_000 } satisfies Lease }; }
      if (path.includes("/interactive-clients/")) return options.method === "DELETE" ? undefined : { data: { client_id: "icl_fixture", session_id: id, expires_at: this.now + 45_000 } };
      if (path.includes("/messages?")) return { data: [], cursor: {} };
      if (path.includes("/permissions/requests")) return { data: this.requests.filter((request) => request.kind === "permission") };
      if (path.includes("/questions/requests")) return { data: this.requests.filter((request) => request.kind === "question") };
      if (path.endsWith("/reply")) {
        if (this.commitReply || !this.failReply) this.requests = [];
        if (this.failReply) { this.failReply = false; throw new ApiFault("network"); }
        return undefined;
      }
      if (path.endsWith("/prompt")) { if (this.failPrompt) { this.failPrompt = false; throw new ApiFault("network"); } return { data: { message_id: "msg_prompt" } }; }
      if (path.endsWith("/diff")) return { data: [] };
      if (path.endsWith("/goals")) return { data: { queue: [], ended: [], current: null } };
      if (path.startsWith("/workflows/runs?") || path.startsWith("/jobs?")) return { data: [], cursor: {} };
      return { data: [] };
    };
    return result() as T;
  }
  async *stream(_path: string, options: StreamOptions): AsyncIterable<EventEnvelope> { options.opened(); await new Promise<void>((resolve) => { if (options.signal.aborted) resolve(); else options.signal.addEventListener("abort", () => resolve(), { once: true }); }); }
}
async function ready(t: { after: (callback: () => void) => void }, api = new Fake()) { const model = new Dashboard(api, () => api.now); t.after(() => model.logout()); await model.login("fixture-only"); await until(() => model.actionable); return { model, api }; }
const replies = (api: Fake) => api.calls.filter((call) => call.path.endsWith("/reply"));

test("lost reply retries the exact owner/request/key/payload, never the selected parent or a fresh decision", async (t) => {
  const { model, api } = await ready(t); const card = model.state.cards[0]!;
  api.failReply = true; await model.answer(card, { reply: "once" });
  assert.equal(card.decision?.status, "uncertain"); assert.equal(replies(api).length, 1);
  model.wake(); await until(() => model.actionable);
  await model.answer(card, { reply: "always" }, true);
  const [first, retry] = replies(api); assert.equal(first!.path, "/sessions/ses_child/permissions/req_one/reply");
  assert.equal(retry!.path, first!.path); assert.equal(retry!.options.key, first!.options.key); assert.deepEqual(retry!.options.body, { reply: "once" });
});

test("another client settles an uncertain answer; reconciliation removes it without replaying into a new request", async (t) => {
  const { model, api } = await ready(t); const old = model.state.cards[0]!; api.failReply = true; api.commitReply = true;
  await model.answer(old, { reply: "once" }); model.wake(); await until(() => model.actionable && !model.state.cards.length);
  api.requests = [permission("req_new")]; await model.refresh();
  await model.answer(old, { reply: "once" }, true);
  assert.equal(replies(api).length, 1); assert.equal(model.state.cards[0]!.request.id, "req_new");
});

test("same request ID with changed immutable payload invalidates old decisions and saved-scope review", async (t) => {
  const { model, api } = await ready(t); const old = model.state.cards[0]!;
  api.requests = [{ ...permission(), metadata: { command: "different command" } }]; await model.refresh();
  await model.answer(old, { reply: "once" }); assert.equal(replies(api).length, 0);
  const current = model.state.cards[0]!; await model.answer(current, { reply: "always" }); assert.equal(replies(api).length, 0);
  model.review(current); model.state.reviewed = true; await model.answer(current, { reply: "always" }); assert.equal(replies(api).length, 1);
});

test("expired/offline cards cannot submit and server restart erases state and requests a new login", async (t) => {
  const { model, api } = await ready(t); const card = model.state.cards[0]!;
  model.state.connection = "offline"; await model.answer(card, { reply: "once" }); assert.equal(replies(api).length, 0);
  model.state.connection = "connected"; api.now += 46_000; await model.answer(card, { reply: "once" }); assert.equal(replies(api).length, 0);
  model.wake(); assert.equal(model.state.authenticated, false); assert.equal(model.state.cards.length, 0);
  await model.login("fixture-only"); await until(() => model.actionable);
  api.identity = "server-two"; model.wake(); await until(() => !model.state.authenticated); assert.match(model.state.notice, /restarted/);
});

test("first lease acknowledgement failure recovers automatically with the same POST key", async (t) => {
  const api = new Fake(); api.failAttach = true; const model = new Dashboard(api, () => api.now); t.after(() => model.logout());
  await model.login("fixture-only"); await until(() => api.calls.filter((call) => call.path.endsWith("/interactive-clients") && call.options.method === "POST").length > 1 && model.actionable);
  const creates = api.calls.filter((call) => call.path.endsWith("/interactive-clients") && call.options.method === "POST"); assert.equal(creates[0]!.options.key, creates[1]!.options.key);
});

test("prompt uncertainty preserves the original message/delivery/key across edits and retry", async (t) => {
  const { model, api } = await ready(t); api.failPrompt = true; model.state.draft = "original"; model.state.delivery = "queue"; await model.prompt();
  model.state.draft = "different"; model.state.delivery = "steer"; model.wake(); await until(() => model.actionable);
  await model.interrupt(); assert.equal(api.calls.filter((call) => call.path.endsWith("/interrupt")).length, 0);
  await model.retryCommand();
  const calls = api.calls.filter((call) => call.path.endsWith("/prompt")); assert.equal(calls.length, 2); assert.equal(calls[0]!.options.key, calls[1]!.options.key); assert.deepEqual(calls[1]!.options.body, { parts: [{ type: "text", text: "original" }], delivery: "queue" });
});

test("late Job/Location responses cannot reveal a previous context after Session switch or logout", async (t) => {
  const { model, api } = await ready(t);
  let finish!: (value: unknown) => void;
  model.state.jobs = [{ id: "job_old" }] as never;
  api.hook = (path) => path === "/jobs/job_old/output" ? new Promise((resolve) => { finish = resolve; }) : undefined;
  const output = model.jobOutput("job_old"); await model.selectSession("ses_two"); finish({ data: { text: "old private output" } }); await output; assert.equal(model.jobLog, undefined);
  let location!: (value: unknown) => void;
  api.hook = (path) => path === "/location" ? new Promise((resolve) => { location = resolve; }) : undefined;
  const change = model.switchLocation("/old"); model.logout(); location({ data: { directory: "/old", project: { id: "old" } } }); await change; assert.equal(model.state.authenticated, false); assert.equal(model.state.directory, "");
});

test("Session switches and logout release only this page's exact lease IDs", async (t) => {
  const { model, api } = await ready(t); await model.selectSession("ses_two"); model.logout();
  const releases = api.calls.filter((call) => call.options.method === "DELETE"); assert.deepEqual(releases.map((call) => call.path), ["/sessions/ses_one/interactive-clients/icl_fixture", "/sessions/ses_two/interactive-clients/icl_fixture"]);
});

 test("Session selection hides all previous transcript, run and diff evidence before the next response", async (t) => {
  const { model, api } = await ready(t);
  model.state.diffs = [{ file: "previous-secret", additions: 1, deletions: 0, patch: "old patch" }] as never;
  model.state.runs = [{ id: "old-run" }] as never; model.state.jobs = [{ id: "old-job" }] as never;
  let resolve!: (value: unknown) => void;
  api.hook = (path) => path === "/sessions/ses_two" ? new Promise((done) => { resolve = done; }) : undefined;
  const pending = model.selectSession("ses_two");
  assert.equal(model.state.session, undefined); assert.equal(model.state.diffs.length, 0); assert.equal(model.state.runs.length, 0); assert.equal(model.state.jobs.length, 0);
  model.logout(); resolve({ data: { ...api.sessions[0], id: "ses_two" } }); await pending;
  assert.equal(model.state.authenticated, false); assert.equal(model.state.session, undefined);
});
