import { strict as assert } from "node:assert";
import { test } from "node:test";
import { applyPatch, parseUnifiedDiff, reversePatch } from "../../src/extension/diff.js";

const one = (diff: string) => {
  const patch = parseUnifiedDiff(diff)[0];
  assert.ok(patch);
  return patch;
};

test("multi-hunk edits apply by exact context", () => {
  const before = Array.from({ length: 20 }, (_, i) => `line ${i + 1}`).join("\n") + "\n";
  const diff = "--- a/src/app.ts\n+++ b/src/app.ts\n@@ -1,4 +1,4 @@\n line 1\n-line 2\n+LINE 2\n line 3\n line 4\n@@ -15,4 +15,5 @@\n line 15\n line 16\n+inserted\n line 17\n line 18\n";
  const patch = one(diff);
  assert.equal(patch.newPath, "src/app.ts");
  assert.equal(patch.hunks.length, 2);
  const after = applyPatch(before, patch);
  assert.ok(after?.includes("LINE 2\nline 3"));
  assert.ok(after?.includes("line 16\ninserted\nline 17"));
  assert.equal(applyPatch(after ?? "", reversePatch(patch)), before);
});

test("added and deleted files", () => {
  const added = one("--- a/new.txt\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n");
  assert.equal(applyPatch("", added), "one\ntwo\n");
  const deleted = one("--- a/old.txt\n+++ b/old.txt\n@@ -1,2 +0,0 @@\n-one\n-two\n");
  assert.equal(applyPatch("one\ntwo\n", deleted), "");
});

test("CRLF lines keep their carriage returns", () => {
  const patch = one("--- a/w.txt\n+++ b/w.txt\n@@ -1,2 +1,2 @@\n a\r\n-b\r\n+c\r\n");
  assert.equal(applyPatch("a\r\nb\r\n", patch), "a\r\nc\r\n");
});

test("a missing trailing newline is kept or added as the diff says", () => {
  const removed = one("--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+c\n");
  assert.equal(applyPatch("a\nb", removed), "a\nc\n");
  const kept = one("--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+c\n\\ No newline at end of file\n");
  assert.equal(applyPatch("a\nb", kept), "a\nc");
  assert.equal(applyPatch("a\nb\n", kept), undefined, "the old side had a newline");
});

test("a context mismatch does not apply", () => {
  const patch = one("--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n a\n-b\n+c\n");
  assert.equal(applyPatch("a\nchanged\n", patch), undefined);
  assert.equal(applyPatch("", patch), undefined);
});

test("a three-file apply_patch diff parses into three files", () => {
  const diff = ["a", "b", "c"].map((name) => `--- a/${name}.ts\n+++ b/${name}.ts\n@@ -1 +1 @@\n-old\n+new\n`).join("");
  const files = parseUnifiedDiff(diff);
  assert.deepEqual(files.map((file) => file.newPath), ["a.ts", "b.ts", "c.ts"]);
  for (const file of files) assert.equal(applyPatch("old\n", file), "new\n");
});
