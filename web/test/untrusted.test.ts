import assert from "node:assert/strict";
import { test } from "node:test";
import { consequence, noticeLine } from "../src/untrusted.js";

// The same wording as cyber-core `untrusted::notice` (`permissions-modes` → Taint-aware approvals).
test("notice consequences follow the mode and settings", () => {
  for (const mode of ["default", "plan", "accept-edits", "auto"]) assert.equal(consequence(mode), "Risky actions in this run will ask.");
  assert.equal(consequence("dont-ask"), "Risky actions in this run will be denied.");
  assert.equal(consequence("bypass"), "Taint is off in bypass.");
  assert.equal(consequence("bypass", { taint: "signal", bypass: "deny" }), "Risky actions in this run will be denied (bypass).");
  assert.equal(consequence("default", { taint: "off", bypass: "deny" }), undefined);
  const record = { source: "mcp", origin: "mcp:docs/search", detector_version: 1, signals: [{ kind: "exfil_url" as const, excerpt: "x" }, { kind: "override_instructions" as const, excerpt: "y" }], signal_count: 2 };
  assert.equal(noticeLine(record, "default", { taint: "off", bypass: "allow" }), "! Possible prompt injection in mcp result (override_instructions, exfil_url).");
  assert.equal(noticeLine({ source: "mcp", origin: "x", detector_version: 1 }, "default"), undefined);
  assert.equal(noticeLine(undefined, "default"), undefined);
});
