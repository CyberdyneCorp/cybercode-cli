// Hidden tests for impl-patch-apply: table-driven edge cases plus seeded randomized patches
// produced by an LCS diff (apply must reproduce the target; reverse must restore the source).
import { test } from "node:test";
import assert from "node:assert/strict";

import { applyPatch } from "./src/applyPatch.js";

const P = (...lines) => lines.join("\n") + "\n";
const ok = (...report) => ({ ok: true, report });
const mod = (path, ...offsets) => ({ path, action: "modify", offsets });
const fail = (error) => ({ ok: false, error });

function check({ files, patch, options, result, after }) {
  const work = structuredClone(files);
  const actual = applyPatch(work, patch, options);
  assert.deepStrictEqual(actual, result);
  assert.deepStrictEqual(work, after ?? files, "files after applyPatch");
}

const HELLO = P("--- a/f.txt", "+++ b/f.txt", "@@ -1,2 +1,2 @@", " hello", "-world", "+there");
const TEN = Array.from({ length: 10 }, (_, k) => `l${k + 1}\n`).join("");

const CASES = [
  // ------------------------------------------------------------------ format
  {
    name: "omitted counts mean 1",
    files: { "f.txt": "a\nb\nc\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -2 +2 @@", "-b", "+B"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "a\nB\nc\n" },
  },
  {
    name: "omitted old count with explicit new count",
    files: { "f.txt": "a\nb\nc\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -2 +2,2 @@", "-b", "+B1", "+B2"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "a\nB1\nB2\nc\n" },
  },
  {
    name: "section heading after the hunk header",
    files: { "f.txt": "a\nb\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1,2 +1,2 @@ function main() {", " a", "-b", "+c"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "a\nc\n" },
  },
  {
    name: "git preamble lines are ignored",
    files: { "f.txt": "hello\nworld\n" },
    patch: "diff --git a/f.txt b/f.txt\nindex 3b18e51..a042389 100644\n" + HELLO,
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "hello\nthere\n" },
  },
  {
    name: "commentary between hunks is ignored",
    files: { "f.txt": TEN },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1 @@", "-l1", "+L1", "some commentary", "", "@@ -10 +10 @@", "-l10", "+L10"),
    result: ok(mod("f.txt", 0, 0)),
    after: { "f.txt": TEN.replace("l1\n", "L1\n").replace("l10\n", "L10\n") },
  },
  {
    name: "hunk headers and markers before the first file header are ignored",
    files: { "f.txt": "hello\nworld\n" },
    patch: "@@ -1 +1 @@\n\\ stray\nnotes\n" + HELLO,
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "hello\nthere\n" },
  },
  {
    name: "path ends at a tab",
    files: { "f.txt": "hello\nworld\n" },
    patch: P("--- a/f.txt\t2024-01-01 10:00:00", "+++ b/f.txt\t2024-01-02 10:00:00", "@@ -1,2 +1,2 @@", " hello", "-world", "+there"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "hello\nthere\n" },
  },
  {
    name: "paths without a/ b/ prefixes",
    files: { "dir/f.txt": "x\n" },
    patch: P("--- dir/f.txt", "+++ dir/f.txt", "@@ -1 +1 @@", "-x", "+y"),
    result: ok(mod("dir/f.txt", 0)),
    after: { "dir/f.txt": "y\n" },
  },
  {
    name: "only one a/ prefix is removed",
    files: { "a/f.txt": "x\n" },
    patch: P("--- a/a/f.txt", "+++ b/a/f.txt", "@@ -1 +1 @@", "-x", "+y"),
    result: ok(mod("a/f.txt", 0)),
    after: { "a/f.txt": "y\n" },
  },
  {
    name: "mixed a/ on both headers",
    files: { "f.txt": "x\n" },
    patch: P("--- a/f.txt", "+++ a/f.txt", "@@ -1 +1 @@", "-x", "+y"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "y\n" },
  },
  {
    name: "CRLF content is preserved",
    files: { "w.txt": "one\r\ntwo\r\nthree\r\n" },
    patch: P("--- a/w.txt", "+++ b/w.txt", "@@ -1,3 +1,3 @@", " one\r", "-two\r", "+TWO\r", " three\r"),
    result: ok(mod("w.txt", 0)),
    after: { "w.txt": "one\r\nTWO\r\nthree\r\n" },
  },
  {
    name: "CRLF patch headers",
    files: { "w.txt": "one\r\ntwo\r\n" },
    patch: "--- a/w.txt\r\n+++ b/w.txt\r\n@@ -1,2 +1,2 @@\r\n one\r\n-two\r\n+2\r\n",
    result: ok(mod("w.txt", 0)),
    after: { "w.txt": "one\r\n2\r\n" },
  },
  {
    name: "LF patch does not match CRLF file",
    files: { "w.txt": "one\r\ntwo\r\n" },
    patch: P("--- a/w.txt", "+++ b/w.txt", "@@ -1,2 +1,2 @@", " one", "-two", "+2"),
    result: fail("hunk 1 of w.txt failed"),
  },
  {
    name: "adding a carriage return is a content change",
    files: { "w.txt": "one\ntwo\n" },
    patch: P("--- a/w.txt", "+++ b/w.txt", "@@ -2 +2 @@", "-two", "+two\r"),
    result: ok(mod("w.txt", 0)),
    after: { "w.txt": "one\ntwo\r\n" },
  },
  {
    name: "empty line in a hunk is an empty context line",
    files: { "f.txt": "a\n\nb\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1,3 +1,3 @@", " a", "", "-b", "+c"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "a\n\nc\n" },
  },
  {
    name: "empty context line as the last line of the patch",
    files: { "f.txt": "a\n\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1,2 +1,2 @@", "-a", "+b", ""),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "b\n\n" },
  },
  {
    name: "removed line that looks like a file header",
    files: { "f.txt": "-- x\nkeep\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1,2 +1,2 @@", "--- x", "+++ y", " keep"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "++ y\nkeep\n" },
  },
  {
    name: "added line that looks like a hunk header",
    files: { "f.txt": "a\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1,2 @@", " a", "+@@ -1 +1 @@"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "a\n@@ -1 +1 @@\n" },
  },
  {
    name: "patch text without a final newline",
    files: { "f.txt": "hello\nworld\n" },
    patch: HELLO.slice(0, -1),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "hello\nthere\n" },
  },
  {
    name: "blank lines after the last hunk are ignored",
    files: { "f.txt": "hello\nworld\n" },
    patch: HELLO + "\n\n",
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "hello\nthere\n" },
  },
  // ------------------------------------------------------------------ no-newline marker
  {
    name: "old side without final newline",
    files: { "f.txt": "a\nb" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1,2 +1,2 @@", " a", "-b", "\\ No newline at end of file", "+c"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "a\nc\n" },
  },
  {
    name: "new side without final newline",
    files: { "f.txt": "a\nb\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1,2 +1,2 @@", " a", "-b", "+b", "\\ No newline at end of file"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "a\nb" },
  },
  {
    name: "both sides without final newline",
    files: { "f.txt": "a\nb" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -2 +2 @@", "-b", "\\ No newline at end of file", "+c", "\\ No newline at end of file"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "a\nc" },
  },
  {
    name: "context line without final newline",
    files: { "f.txt": "a\nb" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1,2 +1,3 @@", " a", "+x", " b", "\\ No newline at end of file"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "a\nx\nb" },
  },
  {
    name: "marker text is arbitrary",
    files: { "f.txt": "a\nb" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -2 +2 @@", "-b", "\\ Kein Zeilenumbruch am Dateiende.", "+c"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "a\nc\n" },
  },
  {
    name: "line with newline does not match a final line without one",
    files: { "f.txt": "a\nb" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -2 +2 @@", "-b", "+c"),
    result: fail("hunk 1 of f.txt failed"),
  },
  {
    name: "line without newline does not match a line with one",
    files: { "f.txt": "a\nb\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -2 +2 @@", "-b", "\\ No newline at end of file", "+c"),
    result: fail("hunk 1 of f.txt failed"),
  },
  {
    name: "marker after the last line of a hunk followed by another hunk header",
    files: { "f.txt": "a\nb\nc" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1 @@", "-a", "+A", "@@ -3 +3 @@", "-c", "\\ No newline at end of file", "+C", "\\ No newline at end of file"),
    result: ok(mod("f.txt", 0, 0)),
    after: { "f.txt": "A\nb\nC" },
  },
  // ------------------------------------------------------------------ create / delete
  {
    name: "create a file",
    files: { "keep.txt": "k\n" },
    patch: P("--- /dev/null", "+++ b/new.txt", "@@ -0,0 +1,2 @@", "+one", "+two"),
    result: ok({ path: "new.txt", action: "create", offsets: [0] }),
    after: { "keep.txt": "k\n", "new.txt": "one\ntwo\n" },
  },
  {
    name: "create a file without final newline",
    files: {},
    patch: P("--- /dev/null", "+++ b/new.txt", "@@ -0,0 +1 @@", "+only", "\\ No newline at end of file"),
    result: ok({ path: "new.txt", action: "create", offsets: [0] }),
    after: { "new.txt": "only" },
  },
  {
    name: "delete a file",
    files: { "old.txt": "one\ntwo\n", "keep.txt": "k\n" },
    patch: P("--- a/old.txt", "+++ /dev/null", "@@ -1,2 +0,0 @@", "-one", "-two"),
    result: ok({ path: "old.txt", action: "delete", offsets: [0] }),
    after: { "keep.txt": "k\n" },
  },
  {
    name: "create an existing file",
    files: { "new.txt": "" },
    patch: P("--- /dev/null", "+++ b/new.txt", "@@ -0,0 +1 @@", "+x"),
    result: fail("new.txt already exists"),
  },
  {
    name: "delete a missing file",
    files: {},
    patch: P("--- a/old.txt", "+++ /dev/null", "@@ -1 +0,0 @@", "-x"),
    result: fail("old.txt does not exist"),
  },
  {
    name: "modify a missing file",
    files: { "other.txt": "x\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1 @@", "-x", "+y"),
    result: fail("f.txt does not exist"),
  },
  {
    name: "delete leaves content behind",
    files: { "old.txt": "one\ntwo\n" },
    patch: P("--- a/old.txt", "+++ /dev/null", "@@ -1 +0,0 @@", "-one"),
    result: fail("old.txt is not empty after deletion"),
  },
  {
    name: "modify an empty file",
    files: { "e.txt": "" },
    patch: P("--- a/e.txt", "+++ b/e.txt", "@@ -0,0 +1 @@", "+x"),
    result: ok(mod("e.txt", 0)),
    after: { "e.txt": "x\n" },
  },
  // ------------------------------------------------------------------ parse errors
  {
    name: "both paths /dev/null",
    files: {},
    patch: P("junk", "--- /dev/null", "+++ /dev/null", "@@ -0,0 +1 @@", "+x"),
    result: fail("line 2: both paths are /dev/null"),
  },
  {
    name: "renames are rejected",
    files: { "a.txt": "x\n" },
    patch: P("--- a/a.txt", "+++ b/b.txt", "@@ -1 +1 @@", "-x", "+y"),
    result: fail("line 1: renames are not supported"),
  },
  {
    name: "missing +++ header",
    files: { "f.txt": "x\n" },
    patch: P("intro", "--- a/f.txt", "index 123", "+++ b/f.txt", "@@ -1 +1 @@", "-x", "+y"),
    result: fail("line 2: missing +++ header"),
  },
  {
    name: "--- on the last line",
    files: { "f.txt": "hello\nworld\n" },
    patch: HELLO + "--- a/g.txt\n",
    result: fail("line 7: missing +++ header"),
  },
  {
    name: "file section without hunks followed by another section",
    files: { "f.txt": "hello\nworld\n", "g.txt": "x\n" },
    patch: P("--- a/g.txt", "+++ b/g.txt", "notes") + HELLO,
    result: fail("line 1: file has no hunks"),
  },
  {
    name: "file section without hunks at the end",
    files: { "f.txt": "hello\nworld\n", "g.txt": "x\n" },
    patch: HELLO + P("--- a/g.txt", "+++ b/g.txt"),
    result: fail("line 7: file has no hunks"),
  },
  {
    name: "missing hunks are reported before a malformed next header",
    files: {},
    patch: P("--- a/g.txt", "+++ b/g.txt", "--- a/h.txt", "junk"),
    result: fail("line 1: file has no hunks"),
  },
  { name: "empty patch", files: { "f.txt": "x\n" }, patch: "", result: fail("no file headers found") },
  {
    name: "patch with only commentary",
    files: {},
    patch: P("just some text", "@@ -1 +1 @@", "+x"),
    result: fail("no file headers found"),
  },
  {
    name: "hunk header without closing @@",
    files: { "f.txt": "x\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1", "-x", "+y"),
    result: fail("line 3: malformed hunk header"),
  },
  {
    name: "hunk header with a non-numeric start",
    files: { "f.txt": "x\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -a +1 @@", "-x", "+y"),
    result: fail("line 3: malformed hunk header"),
  },
  {
    name: "hunk header with an empty count",
    files: { "f.txt": "x\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1, +1 @@", "-x", "+y"),
    result: fail("line 3: malformed hunk header"),
  },
  {
    name: "start 0 with a non-zero count",
    files: { "f.txt": "x\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -0,1 +1 @@", "-x", "+y"),
    result: fail("line 3: malformed hunk header"),
  },
  {
    name: "new start 0 with an omitted count",
    files: { "f.txt": "x\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +0 @@", "-x"),
    result: fail("line 3: malformed hunk header"),
  },
  {
    name: "truncated hunk reports the header line",
    files: { "f.txt": TEN },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1 @@", "-l1", "+L1", "@@ -5,3 +5,3 @@", " l5", "-l6"),
    result: fail("line 6: hunk truncated"),
  },
  {
    name: "removed line beyond the old count",
    files: { "f.txt": "a\nb\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1,2 @@", "-a", "-b", "+c"),
    result: fail("line 5: hunk line exceeds header counts"),
  },
  {
    name: "context line when the new side is complete",
    files: { "f.txt": "a\nb\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1,2 +1 @@", "-a", "+A", " b"),
    result: fail("line 6: hunk line exceeds header counts"),
  },
  {
    name: "added line beyond the new count",
    files: { "f.txt": "a\nb\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1,2 +1 @@", "+A", "+B"),
    result: fail("line 5: hunk line exceeds header counts"),
  },
  {
    name: "unexpected line in a hunk",
    files: { "f.txt": "a\nb\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1,2 +1,2 @@", " a", "*b", "+c"),
    result: fail("line 5: unexpected line in hunk"),
  },
  {
    name: "a file header inside an unfinished hunk is a removed line",
    files: { "f.txt": "a\nb\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1,2 +1,2 @@", " a", "--- a/g.txt", "+++ b/g.txt"),
    result: fail("hunk 1 of f.txt failed"),
  },
  {
    name: "marker right after the hunk header",
    files: { "f.txt": "a\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1 @@", "\\ No newline at end of file", "-a", "+b"),
    result: fail("line 4: misplaced no-newline marker"),
  },
  {
    name: "marker after a line that is not the last old line",
    files: { "f.txt": "a\nb\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1,2 +1 @@", "-a", "\\ No newline at end of file", "-b", "+c"),
    result: fail("line 5: misplaced no-newline marker"),
  },
  {
    name: "marker after a context line that is not the last new line",
    files: { "f.txt": "a" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1,2 @@", " a", "\\ No newline at end of file", "+b"),
    result: fail("line 5: misplaced no-newline marker"),
  },
  {
    name: "marker after an added line that is not the last new line",
    files: { "f.txt": "a\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1,2 @@", "-a", "+b", "\\ No newline at end of file", "+c"),
    result: fail("line 6: misplaced no-newline marker"),
  },
  {
    name: "two markers in a row",
    files: { "f.txt": "a\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1 @@", "-a", "+b", "\\ No newline at end of file", "\\ No newline at end of file"),
    result: fail("line 7: misplaced no-newline marker"),
  },
  {
    name: "marker between hunks",
    files: { "f.txt": "a\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1 @@", "-a", "+b", "comment", "\\ No newline at end of file"),
    result: fail("line 7: misplaced no-newline marker"),
  },
  {
    name: "a later parse error wins over an earlier failing hunk",
    files: { "f.txt": "zzz\n", "g.txt": "x\n" },
    patch: HELLO + P("--- a/g.txt", "+++ b/g.txt", "@@ -1 +1 @@", "-x"),
    result: fail("line 9: hunk truncated"),
  },
  {
    name: "parse errors are reported in reading order",
    files: { "f.txt": "x\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1 @@", "?x", "@@ nonsense"),
    result: fail("line 4: unexpected line in hunk"),
  },
  // ------------------------------------------------------------------ locating hunks
  {
    name: "positive offset",
    files: { "f.txt": "new1\nnew2\nhello\nworld\n" },
    patch: HELLO,
    result: ok(mod("f.txt", 2)),
    after: { "f.txt": "new1\nnew2\nhello\nthere\n" },
  },
  {
    name: "negative offset",
    files: { "f.txt": TEN },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -6,2 +6,2 @@", " l4", "-l5", "+L5"),
    result: ok(mod("f.txt", -2)),
    after: { "f.txt": TEN.replace("l5\n", "L5\n") },
  },
  {
    name: "equal distance prefers the earlier position",
    files: { "f.txt": "q\ny\nx\nq\nq\ny\nx\nq\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -4,2 +4,2 @@", " y", "-x", "+X"),
    result: ok(mod("f.txt", -2)),
    after: { "f.txt": "q\ny\nX\nq\nq\ny\nx\nq\n" },
  },
  {
    name: "earlier position wins at distance one",
    files: { "f.txt": "a\nm\na\nm\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -2 +2 @@", "-a", "+A"),
    result: ok(mod("f.txt", -1)),
    after: { "f.txt": "A\nm\na\nm\n" },
  },
  {
    name: "stated position wins over nearby matches",
    files: { "f.txt": "a\na\na\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -2 +2 @@", "-a", "+A"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "a\nA\na\n" },
  },
  {
    name: "smaller distance wins over the earlier side",
    files: { "f.txt": "t\nq\nq\nq\nq\nt\nq\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -4 +4 @@", "-t", "+T"),
    result: ok(mod("f.txt", 2)),
    after: { "f.txt": "t\nq\nq\nq\nq\nT\nq\n" },
  },
  {
    name: "maxOffset bounds the search",
    files: { "f.txt": "n1\nn2\nn3\nhello\nworld\n" },
    patch: HELLO,
    options: { maxOffset: 2 },
    result: fail("hunk 1 of f.txt failed"),
  },
  {
    name: "offset equal to maxOffset is found",
    files: { "f.txt": "n1\nn2\nn3\nhello\nworld\n" },
    patch: HELLO,
    options: { maxOffset: 3 },
    result: ok(mod("f.txt", 3)),
    after: { "f.txt": "n1\nn2\nn3\nhello\nthere\n" },
  },
  {
    name: "maxOffset 0 only tries the stated position",
    files: { "f.txt": "n1\nhello\nworld\n" },
    patch: HELLO,
    options: { maxOffset: 0 },
    result: fail("hunk 1 of f.txt failed"),
  },
  {
    name: "default maxOffset is 100",
    files: { "f.txt": "n\n".repeat(100) + "hello\nworld\n" },
    patch: HELLO,
    result: ok(mod("f.txt", 100)),
    after: { "f.txt": "n\n".repeat(100) + "hello\nthere\n" },
  },
  {
    name: "offset 101 is beyond the default maxOffset",
    files: { "f.txt": "n\n".repeat(101) + "hello\nworld\n" },
    patch: HELLO,
    result: fail("hunk 1 of f.txt failed"),
  },
  {
    name: "search continues from the previous hunk's offset",
    files: { "f.txt": "n1\nn2\nn3\n" + TEN.replace("l5\n", "l5\nm1\nm2\n") },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1 @@", "-l1", "+L1", "@@ -8 +8 @@", "-l8", "+L8"),
    options: { maxOffset: 3 },
    result: ok(mod("f.txt", 3, 5)),
    after: { "f.txt": "n1\nn2\nn3\n" + TEN.replace("l5\n", "l5\nm1\nm2\n").replace("l1\n", "L1\n").replace("l8\n", "L8\n") },
  },
  {
    name: "previous offset decides between ambiguous matches",
    files: { "f.txt": "s\ns\ns\ns\nh\nd\nd\nd\nd\nd\nd\nd\nd\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1 @@", "-h", "+H", "@@ -6 +6 @@", "-d", "+D"),
    result: ok(mod("f.txt", 4, 4)),
    after: { "f.txt": "s\ns\ns\ns\nH\nd\nd\nd\nd\nD\nd\nd\nd\n" },
  },
  {
    name: "hunks never overlap",
    files: { "f.txt": "A\nB\nA\nB\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1,2 +1,2 @@", " A", "-B", "+C", "@@ -1,2 +1,2 @@", " A", "-B", "+D"),
    result: ok(mod("f.txt", 0, 2)),
    after: { "f.txt": "A\nC\nA\nD\n" },
  },
  {
    name: "hunks never go backwards",
    files: { "f.txt": "a\nb\nc\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -3 +3 @@", "-c", "+C", "@@ -1 +1 @@", "-a", "+A"),
    result: fail("hunk 2 of f.txt failed"),
  },
  {
    name: "adjacent hunks",
    files: { "f.txt": "a\nb\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1 @@", "-a", "+A", "@@ -2 +2 @@", "-b", "+B"),
    result: ok(mod("f.txt", 0, 0)),
    after: { "f.txt": "A\nB\n" },
  },
  {
    name: "pure insertion after a line",
    files: { "f.txt": "a\nb\nc\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -2,0 +3,2 @@", "+x", "+y"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "a\nb\nx\ny\nc\n" },
  },
  {
    name: "pure insertion at the start",
    files: { "f.txt": "a\nb\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -0,0 +1 @@", "+x"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "x\na\nb\n" },
  },
  {
    name: "pure insertion at the end",
    files: { "f.txt": "a\nb\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -2,0 +3 @@", "+x"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "a\nb\nx\n" },
  },
  {
    name: "pure insertion past the end is moved back",
    files: { "f.txt": "a\nb\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -4,0 +5 @@", "+x"),
    result: ok(mod("f.txt", -2)),
    after: { "f.txt": "a\nb\nx\n" },
  },
  {
    name: "pure deletion of a middle line",
    files: { "f.txt": "a\nb\nc\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -2 +1,0 @@", "-b"),
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "a\nc\n" },
  },
  {
    name: "context must match exactly",
    files: { "f.txt": "hello \nworld\n" },
    patch: HELLO,
    result: fail("hunk 1 of f.txt failed"),
  },
  {
    name: "no fuzz on a single differing context line",
    files: { "f.txt": "a\nb\nc\nd\ne\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1,5 +1,5 @@", " a", " b", "-c", "+C", " d", " E"),
    result: fail("hunk 1 of f.txt failed"),
  },
  {
    name: "hunk index counts within its file section",
    files: { "f.txt": "hello\nworld\n", "g.txt": TEN },
    patch: HELLO + P("--- a/g.txt", "+++ b/g.txt", "@@ -1 +1 @@", "-l1", "+L1", "@@ -3 +3 @@", "-l3", "+L3", "@@ -5 +5 @@", "-l9", "+L9", "@@ -7 +7 @@", "-l7", "+L7"),
    options: { maxOffset: 3 },
    result: fail("hunk 3 of g.txt failed"),
  },
  // ------------------------------------------------------------------ atomicity and sequencing
  {
    name: "failure in the second file leaves the first untouched",
    files: { "f.txt": "hello\nworld\n", "g.txt": "x\n" },
    patch: HELLO + P("--- a/g.txt", "+++ b/g.txt", "@@ -1 +1 @@", "-nope", "+y"),
    result: fail("hunk 1 of g.txt failed"),
  },
  {
    name: "failure after a delete keeps the deleted file",
    files: { "old.txt": "x\n", "g.txt": "y\n" },
    patch: P("--- a/old.txt", "+++ /dev/null", "@@ -1 +0,0 @@", "-x", "--- a/g.txt", "+++ b/g.txt", "@@ -1 +1 @@", "-z", "+w"),
    result: fail("hunk 1 of g.txt failed"),
  },
  {
    name: "failure after a create does not add the file",
    files: { "g.txt": "y\n" },
    patch: P("--- /dev/null", "+++ b/new.txt", "@@ -0,0 +1 @@", "+n", "--- a/g.txt", "+++ b/g.txt", "@@ -1 +1 @@", "-z", "+w"),
    result: fail("hunk 1 of g.txt failed"),
  },
  {
    name: "failure in a later hunk of the same file",
    files: { "f.txt": TEN },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1 @@", "-l1", "+L1", "@@ -9 +9 @@", "-nine", "+NINE"),
    result: fail("hunk 2 of f.txt failed"),
  },
  {
    name: "a missing file after successful sections changes nothing",
    files: { "f.txt": "hello\nworld\n" },
    patch: HELLO + P("--- a/g.txt", "+++ /dev/null", "@@ -1 +0,0 @@", "-x"),
    result: fail("g.txt does not exist"),
  },
  {
    name: "the same file in two sections",
    files: { "f.txt": "hello\nworld\n" },
    patch: HELLO + P("--- a/f.txt", "+++ b/f.txt", "@@ -2 +2 @@", "-there", "+again"),
    result: ok(mod("f.txt", 0), mod("f.txt", 0)),
    after: { "f.txt": "hello\nagain\n" },
  },
  {
    name: "create then modify the same file",
    files: {},
    patch: P("--- /dev/null", "+++ b/n.txt", "@@ -0,0 +1 @@", "+a", "--- a/n.txt", "+++ b/n.txt", "@@ -1 +1,2 @@", " a", "+b"),
    result: ok({ path: "n.txt", action: "create", offsets: [0] }, mod("n.txt", 0)),
    after: { "n.txt": "a\nb\n" },
  },
  {
    name: "delete then create the same file",
    files: { "n.txt": "old\n" },
    patch: P("--- a/n.txt", "+++ /dev/null", "@@ -1 +0,0 @@", "-old", "--- /dev/null", "+++ b/n.txt", "@@ -0,0 +1 @@", "+new"),
    result: ok({ path: "n.txt", action: "delete", offsets: [0] }, { path: "n.txt", action: "create", offsets: [0] }),
    after: { "n.txt": "new\n" },
  },
  {
    name: "delete then modify the same file",
    files: { "n.txt": "old\n" },
    patch: P("--- a/n.txt", "+++ /dev/null", "@@ -1 +0,0 @@", "-old", "--- a/n.txt", "+++ b/n.txt", "@@ -1 +1 @@", "-old", "+new"),
    result: fail("n.txt does not exist"),
  },
  {
    name: "offsets are relative to each section's own content",
    files: { "f.txt": "x\nhello\nworld\n" },
    patch: HELLO + HELLO.replace("-world\n+there", "-there\n+world"),
    result: ok(mod("f.txt", 1), mod("f.txt", 1)),
    after: { "f.txt": "x\nhello\nworld\n" },
  },
  // ------------------------------------------------------------------ reverse
  {
    name: "reverse a modification",
    files: { "f.txt": "hello\nthere\n" },
    patch: HELLO,
    options: { reverse: true },
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "hello\nworld\n" },
  },
  {
    name: "reverse of an unapplied patch fails",
    files: { "f.txt": "hello\nworld\n" },
    patch: HELLO,
    options: { reverse: true },
    result: fail("hunk 1 of f.txt failed"),
  },
  {
    name: "reverse a create deletes the file",
    files: { "new.txt": "one\ntwo\n" },
    patch: P("--- /dev/null", "+++ b/new.txt", "@@ -0,0 +1,2 @@", "+one", "+two"),
    options: { reverse: true },
    result: ok({ path: "new.txt", action: "delete", offsets: [0] }),
    after: {},
  },
  {
    name: "reverse a delete creates the file",
    files: {},
    patch: P("--- a/old.txt", "+++ /dev/null", "@@ -1,2 +0,0 @@", "-one", "-two"),
    options: { reverse: true },
    result: ok({ path: "old.txt", action: "create", offsets: [0] }),
    after: { "old.txt": "one\ntwo\n" },
  },
  {
    name: "reverse a create of an existing-but-different file",
    files: { "new.txt": "other\n" },
    patch: P("--- /dev/null", "+++ b/new.txt", "@@ -0,0 +1 @@", "+one"),
    options: { reverse: true },
    result: fail("hunk 1 of new.txt failed"),
  },
  {
    name: "reverse with no-newline markers",
    files: { "f.txt": "a\nc\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1,2 +1,2 @@", " a", "-b", "\\ No newline at end of file", "+c"),
    options: { reverse: true },
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "a\nb" },
  },
  {
    name: "reverse uses the new-side start",
    files: { "f.txt": "x\ny\nz\na\nB\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1,2 +4,2 @@", " a", "-b", "+B"),
    options: { reverse: true },
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "x\ny\nz\na\nb\n" },
  },
  {
    name: "reverse of an insertion is a deletion at the new position",
    files: { "f.txt": "a\nb\nx\ny\nc\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -2,0 +3,2 @@", "+x", "+y"),
    options: { reverse: true },
    result: ok(mod("f.txt", 0)),
    after: { "f.txt": "a\nb\nc\n" },
  },
  {
    name: "reverse reports offsets",
    files: { "f.txt": "p\np\nhello\nthere\n" },
    patch: HELLO,
    options: { reverse: true },
    result: ok(mod("f.txt", 2)),
    after: { "f.txt": "p\np\nhello\nworld\n" },
  },
  {
    name: "reverse still parses the patch as written",
    files: { "f.txt": "a\n" },
    patch: P("--- a/f.txt", "+++ b/f.txt", "@@ -1 +1 @@", "-a", "-b", "+c"),
    options: { reverse: true },
    result: fail("line 5: hunk line exceeds header counts"),
  },
  {
    name: "reverse across files is atomic",
    files: { "f.txt": "hello\nthere\n", "g.txt": "q\n" },
    patch: HELLO + P("--- a/g.txt", "+++ b/g.txt", "@@ -1 +1 @@", "-q", "+r"),
    options: { reverse: true },
    result: fail("hunk 1 of g.txt failed"),
  },
];

for (const c of CASES) test(`table: ${c.name}`, () => check(c));

test("table: success mutates the given object in place", () => {
  const files = { "f.txt": "hello\nworld\n", "keep.txt": "k\n" };
  const keep = files;
  const result = applyPatch(files, HELLO);
  assert.equal(result.ok, true);
  assert.equal(files, keep);
  assert.deepStrictEqual(files, { "f.txt": "hello\nthere\n", "keep.txt": "k\n" });
});

test("table: options default when omitted", () => {
  const files = { "f.txt": "hello\nworld\n" };
  assert.deepStrictEqual(applyPatch(files, HELLO), ok(mod("f.txt", 0)));
});

// ================================================================== randomized
function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function diffOps(a, b) {
  const n = a.length;
  const m = b.length;
  const dp = Array.from({ length: n + 1 }, () => new Int32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }
  const ops = [];
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && a[i] === b[j]) ops.push({ t: " ", a: i++, b: j++, line: a[i - 1] });
    else if (i < n && (j === m || dp[i + 1][j] >= dp[i][j + 1])) ops.push({ t: "-", a: i++, b: j, line: a[i - 1] });
    else ops.push({ t: "+", a: i, b: j++, line: b[j - 1] });
  }
  return ops;
}

/** Hunks of a unified diff of line arrays a -> b with `context` lines of context. */
function diffHunks(a, b, context) {
  const ops = diffOps(a, b);
  const groups = [];
  for (const [k, op] of ops.entries()) {
    if (op.t === " ") continue;
    const last = groups.at(-1);
    if (last && k - last[1] - 1 <= 2 * context) last[1] = k;
    else groups.push([k, k]);
  }
  return groups.map(([s, e]) => {
    const slice = ops.slice(Math.max(0, s - context), Math.min(ops.length, e + context + 1));
    const oc = slice.filter((o) => o.t !== "+").length;
    const nc = slice.filter((o) => o.t !== "-").length;
    const p0 = slice[0].a;
    const range = (start, count) => (count === 1 ? `${start}` : `${start},${count}`);
    const lines = [`@@ -${range(oc ? p0 + 1 : p0, oc)} +${range(nc ? slice[0].b + 1 : slice[0].b, nc)} @@`];
    for (const o of slice) {
      const nl = o.line.endsWith("\n");
      lines.push(o.t + (nl ? o.line.slice(0, -1) : o.line));
      if (!nl) lines.push("\\ No newline at end of file");
    }
    return { p0, oc, nc, text: lines.join("\n") + "\n" };
  });
}

const split = (s) => s.match(/[^\n]*\n|[^\n]+$/g) ?? [];

/** File section text for path: before/after are contents, or null for create/delete. */
function fileSection(path, before, after, context, preamble) {
  const hunks = diffHunks(split(before ?? ""), split(after ?? ""), context);
  const head = preamble ? `diff --git a/${path} b/${path}\nindex 1a2b3c4..5d6e7f8 100644\n` : "";
  const oldName = before === null ? "/dev/null" : `a/${path}`;
  const newName = after === null ? "/dev/null" : `b/${path}`;
  return { hunks, text: `${head}--- ${oldName}\n+++ ${newName}\n${hunks.map((h) => h.text).join("")}` };
}

function makeGen(seed) {
  const r = rng(seed);
  const int = (lo, hi) => lo + Math.floor(r() * (hi - lo + 1));
  const pick = (xs) => xs[int(0, xs.length - 1)];
  let uid = 0;
  const gen = {
    r, int, pick,
    unique: () => `u${seed}_${uid++}`,
    /** Random content: `alphabet` lines (null = unique lines), CRLF and final newline at random. */
    content(count, alphabet = null, eol = pick(["\n", "\n", "\r\n"]), finalNewline = r() < 0.75) {
      const lines = Array.from({ length: count }, () => (alphabet ? pick(alphabet) : gen.unique()));
      let text = lines.map((l) => l + eol).join("");
      if (!finalNewline && text) text = text.slice(0, -1);
      return text;
    },
    /** A random edit of content (same line style). */
    edit(text, alphabet = null) {
      const eol = text.includes("\r\n") ? "\r\n" : "\n";
      const lines = split(text).map((l) => l.replace(/\r?\n$/, ""));
      const finalNewline = text === "" || text.endsWith("\n");
      const out = [];
      const fresh = () => (alphabet ? pick(alphabet) : gen.unique());
      for (const line of lines) {
        const roll = r();
        if (roll < 0.12) continue;
        if (roll < 0.22) out.push(fresh());
        else out.push(line);
        if (r() < 0.1) for (let k = int(1, 3); k > 0; k--) out.push(fresh());
      }
      if (r() < 0.2) out.unshift(fresh());
      if (out.length === 0) out.push(fresh());
      const flip = r() < 0.15;
      let result = out.map((l) => l + eol).join("");
      if (finalNewline === flip) result = result.slice(0, -1);
      return result;
    },
  };
  return gen;
}

const ALPHABET = ["a", "b", "c", "foo", "bar", "", "  x", "}"];

for (let seed = 1; seed <= 24; seed++) {
  test(`random: modify with repeated lines (seed ${seed})`, () => {
    const g = makeGen(seed);
    const source = g.content(g.int(1, 40), ALPHABET);
    let target = g.edit(source, ALPHABET);
    if (target === source) target = source + "tail\n";
    const context = seed % 4;
    const { hunks, text } = fileSection("src/f.txt", source, target, context, seed % 2 === 0);
    const zeros = hunks.map(() => 0);
    const files = { "src/f.txt": source, "other.txt": "o\n" };
    assert.deepStrictEqual(applyPatch(files, text), ok(mod("src/f.txt", ...zeros)));
    assert.deepStrictEqual(files, { "src/f.txt": target, "other.txt": "o\n" });
    assert.deepStrictEqual(applyPatch(files, text, { reverse: true }), ok(mod("src/f.txt", ...zeros)));
    assert.deepStrictEqual(files, { "src/f.txt": source, "other.txt": "o\n" });
  });
}

/** A shiftable case: unique lines, extra lines inserted at an index outside every hunk. */
function shiftCase(seed) {
  const g = makeGen(seed);
  for (;;) {
    const source = g.content(g.int(8, 60));
    const target = g.edit(source);
    const context = g.int(1, 3);
    const { hunks, text } = fileSection("m.txt", source, target, context, false);
    if (hunks.length === 0) continue;
    const at = g.pick(hunks.map((h) => h.p0));
    const firstAfter = hunks.findIndex((h) => h.p0 >= at);
    const delta = hunks.slice(0, firstAfter).reduce((sum, h) => sum + h.nc - h.oc, 0);
    return { g, source, target, hunks, text, at, firstAfter, delta };
  }
}

function insertLines(text, index, extra) {
  const lines = split(text);
  if (index === lines.length && text !== "" && !text.endsWith("\n")) throw new Error("unsupported");
  return [...lines.slice(0, index), ...extra, ...lines.slice(index)].join("");
}

for (let seed = 100; seed < 118; seed++) {
  test(`random: offsets after inserted lines (seed ${seed})`, () => {
    const { g, source, target, hunks, text, at, firstAfter, delta } = shiftCase(seed);
    const eol = source.includes("\r\n") ? "\r\n" : "\n";
    const extra = Array.from({ length: g.int(1, 20) }, () => g.unique() + eol);
    const files = { "m.txt": insertLines(source, at, extra) };
    const offsets = hunks.map((_, k) => (k < firstAfter ? 0 : extra.length));
    assert.deepStrictEqual(applyPatch(files, text), ok(mod("m.txt", ...offsets)));
    assert.deepStrictEqual(files, { "m.txt": insertLines(target, at + delta, extra) });
  });
}

for (let seed = 200; seed < 206; seed++) {
  test(`random: shift beyond maxOffset fails (seed ${seed})`, () => {
    const { g, source, text, at, firstAfter } = shiftCase(seed);
    const maxOffset = g.int(0, 5);
    const extra = Array.from({ length: maxOffset + g.int(1, 4) }, () => g.unique() + "\n");
    const files = { "m.txt": insertLines(source, at, extra) };
    const before = structuredClone(files);
    assert.deepStrictEqual(applyPatch(files, text, { maxOffset }), fail(`hunk ${firstAfter + 1} of m.txt failed`));
    assert.deepStrictEqual(files, before);
  });
}

for (let seed = 300; seed < 312; seed++) {
  test(`random: multi-file create/modify/delete and reverse (seed ${seed})`, () => {
    const g = makeGen(seed);
    const original = { "keep.txt": "untouched\n" };
    const expected = { "keep.txt": "untouched\n" };
    const report = [];
    let patch = "";
    for (let k = 0; k < g.int(2, 5); k++) {
      const path = `dir${k}/file${k}.txt`;
      const kind = g.pick(["modify", "modify", "create", "delete"]);
      const alphabet = g.r() < 0.5 ? ALPHABET : null;
      const before = kind === "create" ? null : g.content(g.int(1, 25), alphabet);
      let after = kind === "delete" ? null : kind === "create" ? g.content(g.int(1, 10), alphabet) : g.edit(before, alphabet);
      if (after === before) after = before + "z\n";
      const section = fileSection(path, before, after, g.int(0, 3), g.r() < 0.5);
      patch += section.text + (g.r() < 0.3 ? "commentary line\n" : "");
      if (before !== null) original[path] = before;
      if (after !== null) expected[path] = after;
      report.push({ path, action: kind, offsets: section.hunks.map(() => 0) });
    }
    const files = structuredClone(original);
    assert.deepStrictEqual(applyPatch(files, patch), { ok: true, report });
    assert.deepStrictEqual(files, expected);
    const inverted = { create: "delete", delete: "create", modify: "modify" };
    const reverseReport = report.map((e) => ({ ...e, action: inverted[e.action] }));
    assert.deepStrictEqual(applyPatch(files, patch, { reverse: true }), { ok: true, report: reverseReport });
    assert.deepStrictEqual(files, original);
  });
}

for (let seed = 400; seed < 412; seed++) {
  test(`random: one corrupted hunk fails the whole patch (seed ${seed})`, () => {
    const g = makeGen(seed);
    const files = {};
    let patch = fileSection("new.txt", null, g.content(g.int(1, 5)), 3, false).text;
    const sections = [];
    for (let k = 0; k < 3; k++) {
      const path = `f${k}.txt`;
      let before;
      let after;
      let section;
      do {
        before = g.content(g.int(5, 40));
        after = g.edit(before);
        section = fileSection(path, before, after, g.int(1, 3), true);
      } while (section.hunks.length === 0);
      files[path] = before;
      sections.push({ path, section });
      patch += section.text;
    }
    const victim = g.int(0, 2);
    const { path, section } = sections[victim];
    const hunkIndex = g.int(0, section.hunks.length - 1);
    const hunk = section.hunks[hunkIndex];
    const lines = split(files[path]);
    const target = hunk.p0 + g.int(0, hunk.oc - 1);
    lines[target] = `CORRUPT${lines[target].endsWith("\n") ? "\n" : ""}`;
    files[path] = lines.join("");
    const before = structuredClone(files);
    assert.deepStrictEqual(applyPatch(files, patch), fail(`hunk ${hunkIndex + 1} of ${path} failed`));
    assert.deepStrictEqual(files, before);
  });
}
