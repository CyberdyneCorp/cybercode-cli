import { strict as assert } from "node:assert";
import { test } from "node:test";
import { delimiter } from "node:path";
import { resolveExecutable } from "../../src/extension/executable.js";

const files = new Set(["/opt/cyber/bin/cyber", "/usr/local/bin/cyber"]);
const runnable = (file: string) => files.has(file);

test("the machine setting wins and must be an absolute runnable file", () => {
  const env = { platform: "linux" as const, path: "/usr/local/bin", runnable };
  assert.equal(resolveExecutable("/opt/cyber/bin/cyber", env), "/opt/cyber/bin/cyber");
  assert.equal(resolveExecutable("./tools/cyber", env), undefined);
  assert.equal(resolveExecutable("/missing/cyber", env), undefined);
});

test("otherwise cyber on PATH, skipping relative entries", () => {
  const path = [".", "tools", "/usr/bin", "/usr/local/bin"].join(delimiter);
  assert.equal(resolveExecutable("", { platform: "linux", path, runnable }), "/usr/local/bin/cyber");
  assert.equal(resolveExecutable(undefined, { platform: "linux", path: "/usr/bin", runnable }), undefined);
});
