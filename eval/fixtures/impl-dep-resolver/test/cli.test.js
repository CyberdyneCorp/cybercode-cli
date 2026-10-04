import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";

test("resolves a simple dependency tree", () => {
  const result = spawnSync(
    process.execPath,
    ["src/cli.js", "resolve", "test/fixtures/registry.json", "test/fixtures/root.json"],
    { encoding: "utf8" },
  );
  assert.equal(result.stderr, "");
  assert.equal(result.status, 0);
  assert.deepEqual(JSON.parse(result.stdout), {
    lockfileVersion: 1,
    packages: {
      core: { version: "2.1.0", dependencies: { util: "0.3.1" } },
      "left-pad": { version: "1.1.0", dependencies: { core: "2.1.0" } },
      util: { version: "0.3.1", dependencies: {} },
    },
  });
});
