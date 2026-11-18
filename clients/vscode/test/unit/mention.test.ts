import { strict as assert } from "node:assert";
import { test } from "node:test";
import { filePart, mention, selectedLines } from "../../src/extension/mention.js";

test("Selection reference: lines 12-30 of src/app.ts", () => {
  const lines = selectedLines({ line: 11, character: 0 }, { line: 30, character: 0 });
  assert.deepEqual(lines, { start: 12, end: 30 });
  assert.equal(mention("/repo", "/repo/src/app.ts", lines!), "@src/app.ts#L12-30");
  assert.equal(mention("/repo", "/repo/src/app.ts", { start: 7, end: 7 }), "@src/app.ts#L7");
  assert.equal(mention("/repo", "/elsewhere/x.ts", { start: 1, end: 2 }), "@/elsewhere/x.ts#L1-2");
  assert.equal(selectedLines({ line: 3, character: 2 }, { line: 3, character: 2 }), undefined);
});

test("Selection sent as a ranged file part", () => {
  assert.deepEqual(filePart("/repo", "/repo/src/app.ts", { start: 12, end: 30 }, true), {
    type: "file",
    path: "src/app.ts",
    range: { start_line: 12, end_line: 30 },
  });
  assert.deepEqual(filePart("/repo", "/repo/src/app.ts", undefined, true), { type: "file", path: "src/app.ts" });
});
