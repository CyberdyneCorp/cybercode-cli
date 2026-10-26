import assert from "node:assert/strict";
import { test } from "node:test";
import { CyberApiError, isAutofixNotFoundError } from "../src/index.js";
import { json, mockClient } from "./support.js";

test("generated auto-fix enable preserves owner opt-in, pinned verification and hard limits", async () => {
  const config = {
    repository: "team/repo", pr: 3, session_id: "ses_owner", model: "openai/gpt-4.1",
    checks: [["cargo", "test", "name with spaces"]], required_ci: ["check:9:Rust tests"],
    verification_paths: ["tests", "Cargo.toml"], max_attempts: 3, max_tokens: 2000000, max_seconds: 600,
  };
  const { client, calls } = mockClient(() => json(201, { data: { id: "afx_fixture", status: "enabled" } }));
  const result = await client.at("/project with spaces").autofix.enable(config, { idempotencyKey: "enable-owner-request" });
  assert.equal(result.id, "afx_fixture");
  assert.equal(calls[0]?.url.pathname, "/api/v1/autofix/subscriptions");
  assert.equal(calls[0]?.method, "POST");
  assert.deepEqual(JSON.parse(calls[0]?.body ?? "{}"), config);
  assert.equal(calls[0]?.headers.get("x-cyber-directory"), encodeURIComponent("/project with spaces"));
  assert.equal(calls[0]?.headers.get("idempotency-key"), "enable-owner-request");
  assert.equal(calls[0]?.headers.get("authorization"), `Basic ${btoa("cyber:secret")}`);
});

test("generated auto-fix control keeps revision fences and typed not-found outcomes", async () => {
  const { client, calls } = mockClient(() => json(200, { data: { id: "afx_fixture", status: "disabled" } }));
  await client.autofix.control("afx_fixture", { expected_revision: 7, action: "disable" });
  assert.equal(calls[0]?.url.pathname, "/api/v1/autofix/subscriptions/afx_fixture/control");
  assert.deepEqual(JSON.parse(calls[0]?.body ?? "{}"), { expected_revision: 7, action: "disable" });
  const missing = mockClient(() => json(404, { _tag: "AutofixNotFoundError", message: "missing" }));
  await assert.rejects(missing.client.autofix.show("afx_missing"), (error: unknown) => error instanceof CyberApiError && isAutofixNotFoundError(error));
});
