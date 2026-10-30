import assert from "node:assert/strict";
import { test } from "node:test";
import type { Session } from "../../sdk/typescript/src/generated/types.js";
import { usageLimitNotice } from "../src/usage-limit.js";

const session = (usage_limit: Session["usage_limit"]) => ({ id: "ses_one", status: "running", usage_limit }) as Session;

test("a waiting Session shows when it resumes, and the notice clears with the wait", () => {
  const wait = { message_id: "msg_one", resets_at_ms: 1_000, resume_at_ms: 12_345, started_ms: 0, by: "runtime" as const };
  assert.equal(usageLimitNotice(session(wait), (ms) => `t${ms}`), "Usage limit reached. Resuming at t12345; Interrupt cancels the wait.");
  assert.equal(usageLimitNotice(session(null)), undefined);
  assert.equal(usageLimitNotice(undefined), undefined);
});
