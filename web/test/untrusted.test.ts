import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { NoticePolicy, UntrustedRecord } from "../../sdk/typescript/src/generated/types.js";
import { consequence, noticeLine } from "../src/untrusted.js";

// The same wording as cyber-core `untrusted::notice` (`permissions-modes` → Taint-aware approvals).
test("notice consequences follow the mode and settings", () => {
  for (const mode of ["default", "plan", "accept-edits", "auto"]) assert.equal(consequence(mode), "Risky actions in this run will ask.");
  assert.equal(consequence("dont-ask"), "Risky actions in this run will be denied.");
  assert.equal(consequence("bypass"), "Taint is off in bypass.");
  assert.equal(consequence("bypass", { taint: "signal", bypass: "deny" }), "Risky actions in this run will be denied (bypass).");
  assert.equal(consequence("default", { taint: "off", bypass: "deny" }), undefined);
  assert.equal(noticeLine(undefined, "default"), undefined);
});

interface NoticeCase { name: string; record: UntrustedRecord; mode: string; policy: NoticePolicy; line: string | null }

// Regression: the dashboard re-implemented the wording and had drifted (no markers for hidden
// characters in a source). Both renderings now check the same fixture.
test("notice lines match the fixture cyber-core checks", () => {
  const fixture = JSON.parse(readFileSync(new URL("../../eval/fixtures/untrusted-content/notices.json", import.meta.url), "utf8")) as { cases: NoticeCase[] };
  assert.ok(fixture.cases.length > 5);
  for (const sample of fixture.cases) assert.equal(noticeLine(sample.record, sample.mode, sample.policy) ?? null, sample.line, sample.name);
});
