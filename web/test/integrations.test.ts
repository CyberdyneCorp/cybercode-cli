import assert from "node:assert/strict";
import { test } from "node:test";
import { Integrations } from "../src/integrations.js";
import { ApiFault, type Port, type RequestOptions } from "../src/transport.js";
function setup() {
  const calls: Array<{ path: string; options: RequestOptions }> = [];
  const context = {
    directory: "/project",
    session: "ses_owner",
    enabled: true,
  };
  const batch = {
    id: "intg_exact",
    revision: 7,
    status: "ready",
    manifest: { contributions: [], checks: [], reviewer_model: "verify" },
    report: { host: { eligible: true } },
  };
  const api: Port = {
    password() {},
    clear() {},
    async *stream() {},
    async request<T>(path: string, options: RequestOptions = {}) {
      calls.push({ path, options });
      return {
        data:
          path === "/workflows/integrations" && options.method !== "POST"
            ? [batch]
            : batch,
      } as T;
    },
  };
  return {
    model: new Integrations(
      api,
      () => context,
      () => {},
    ),
    calls,
    context,
    batch,
  };
}
test("integration controls bind exact revision and location; publication needs fresh review", async () => {
  const { model, calls, batch } = setup();
  await model.load(batch.id);
  await model.execute("publish");
  assert.equal(calls.length, 1);
  model.reviewed = true;
  model.publication.repository = "team/repo";
  model.publication.title = "Combine";
  await model.execute("publish");
  const action = calls.find((call) => call.options.method === "POST")!;
  assert.equal(action.path, "/workflows/integrations/intg_exact/publish");
  assert.equal(action.options.directory, "/project");
  assert.equal(
    (action.options.body as { expected_revision: number }).expected_revision,
    7,
  );
  assert.ok(action.options.key);
  assert.equal(model.reviewed, false);
});
test("logout/location reset erases contributions and disables actions", async () => {
  const { model, context } = setup();
  await model.load();
  await model.load("intg_exact");
  assert.equal(model.rows.length, 1);
  context.enabled = false;
  model.reset();
  assert.equal(model.selected, undefined);
  assert.deepEqual(model.rows, []);
  await model.execute("run");
  assert.equal(model.busy, false);
});

test("an interrupted integration action retries the frozen request identity and body", async () => {
  const { model, calls } = setup();
  model.manifest = '{"contributions":[],"checks":[],"reviewer_model":"verify"}';
  const original = model.api.request.bind(model.api);
  let failed = false;
  model.api.request = async <T>(path: string, options: RequestOptions = {}) => {
    if (options.method === "POST" && !failed) {
      failed = true;
      calls.push({ path, options: structuredClone(options) });
      throw new Error("connection interrupted");
    }
    return original<T>(path, options);
  };
  await model.submit();
  assert.equal(model.uncertain, true);
  const first = calls.find((call) => call.options.method === "POST")!;
  model.manifest = '{"different":"input"}';
  await model.submit();
  assert.equal(
    calls.filter((call) => call.options.method === "POST").length,
    1,
  );
  await model.retry();
  const requests = calls.filter((call) => call.options.method === "POST");
  assert.equal(requests.length, 2);
  assert.equal(requests[1]!.options.key, first.options.key);
  assert.deepEqual(requests[1]!.options.body, first.options.body);
  assert.equal(model.uncertain, false);
});

test("a conflict can mean the original request is still running; it retains the retry until explicit refreshed dismissal", async () => {
  const { model } = setup();
  const original = model.api.request.bind(model.api);
  model.api.request = async <T>(path: string, options: RequestOptions = {}) => {
    if (options.method === "POST") throw new ApiFault("changed", 409);
    return original<T>(path, options);
  };
  await model.submit();
  assert.equal(model.uncertain, true);
  assert.match(model.pendingTarget, /owner Session ses_owner/);
  await model.discardRetry();
  assert.equal(model.uncertain, false);
  assert.equal(model.pendingTarget, "");
});
