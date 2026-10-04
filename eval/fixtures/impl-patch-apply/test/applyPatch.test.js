import { test } from "node:test";
import assert from "node:assert/strict";

import { applyPatch } from "../src/applyPatch.js";

const PATCH = [
  "--- a/greeting.txt",
  "+++ b/greeting.txt",
  "@@ -1,2 +1,2 @@",
  " hello",
  "-world",
  "+there",
  "",
].join("\n");

test("applies a single hunk", () => {
  const files = { "greeting.txt": "hello\nworld\n" };
  const result = applyPatch(files, PATCH);
  assert.deepEqual(result, { ok: true, report: [{ path: "greeting.txt", action: "modify", offsets: [0] }] });
  assert.deepEqual(files, { "greeting.txt": "hello\nthere\n" });
});

test("reports a failing hunk", () => {
  const files = { "greeting.txt": "goodbye\nworld\n" };
  const result = applyPatch(files, PATCH);
  assert.deepEqual(result, { ok: false, error: "hunk 1 of greeting.txt failed" });
  assert.deepEqual(files, { "greeting.txt": "goodbye\nworld\n" });
});
