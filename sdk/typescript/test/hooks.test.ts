import assert from "node:assert/strict";
import { test } from "node:test";
import type { HookExecutionRecord } from "../src/generated/types.js";
import { json, mockClient } from "./support.js";

test("session hook receipts preserve pagination, unresolved status and IO omission", async () => {
  const record: HookExecutionRecord = {
    id: "hke_1", session_id: "ses_1", hook_id: "guard", digest: "sha256:reviewed",
    event: "PreToolUse", call_id: "call_1", tool_name: "write", scope: "global",
    directory: "/repo", agent: "build", mode: "default", started_ms: 1,
    log_io: false, once: false, status: "unknown", duration_ms: 2,
    outcome: "error", decision: {}, acknowledged: false, must_stop: true,
  };
  const { client, calls } = mockClient(() => json(200, { data: [record], cursor: { next: "1:ses_1:hke_1" } }), { directory: "/other" });
  const page = await client.session.hooks("ses/1", { limit: 1, cursor: "2:ses_1:hke_2" });
  assert.equal(page.data[0]?.status, "unknown");
  assert.equal(page.data[0]?.call_id, "call_1");
  assert.equal(page.data[0]?.io, undefined);
  assert.equal(page.cursor.next, "1:ses_1:hke_1");
  assert.equal(calls[0]?.url.pathname, "/api/v1/sessions/ses%2F1/hook-executions");
  assert.equal(calls[0]?.url.searchParams.get("cursor"), "2:ses_1:hke_2");
  assert.equal(calls[0]?.url.searchParams.get("limit"), "1");
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), null);
  assert.equal(calls[0]?.headers.get("authorization"), `Basic ${btoa("cyber:secret")}`);
});

test("hook catalog retains Location, withheld definitions and current trust metadata", async () => {
  const { client, calls } = mockClient(() => json(200, {
    location: { directory: "/repo", project: { id: "global", directory: "/repo" } },
    data: { hooks: [], withheld_definitions: ["/repo/cyber.jsonc#/hooks"], checkout_trusted: false },
  }), { directory: "/repo" });
  const review = await client.hook.list();
  assert.equal(review.location.directory, "/repo");
  assert.equal(review.data.checkout_trusted, false);
  assert.deepEqual(review.data.withheld_definitions, ["/repo/cyber.jsonc#/hooks"]);
  assert.equal(calls[0]?.method, "GET");
  assert.equal(calls[0]?.url.pathname, "/api/v1/hooks");
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), encodeURIComponent("/repo"));
  assert.equal(calls[0]?.headers.get("authorization"), `Basic ${btoa("cyber:secret")}`);
});
