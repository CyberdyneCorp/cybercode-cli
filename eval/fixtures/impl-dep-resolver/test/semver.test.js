import { test } from "node:test";
import assert from "node:assert/strict";

import { satisfies } from "../src/semver.js";

test("satisfies exact versions and comparators", () => {
  assert.equal(satisfies("1.2.3", "1.2.3"), true);
  assert.equal(satisfies("1.2.4", "1.2.3"), false);
  assert.equal(satisfies("1.5.0", ">=1.2.3 <2.0.0"), true);
  assert.equal(satisfies("2.0.0", ">=1.2.3 <2.0.0"), false);
  assert.equal(satisfies("1.9.0", "^1.2.3"), true);
});
