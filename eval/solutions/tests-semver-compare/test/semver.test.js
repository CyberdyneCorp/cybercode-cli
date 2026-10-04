import { test } from "node:test";
import assert from "node:assert/strict";

import { compare, parse, sortVersions } from "../src/semver.js";

test("parse splits all parts", () => {
  assert.deepEqual(parse("1.22.333-alpha.1+001.sha"), {
    major: 1, minor: 22, patch: 333, prerelease: ["alpha", "1"], build: ["001", "sha"],
  });
  assert.deepEqual(parse("0.0.0"), { major: 0, minor: 0, patch: 0, prerelease: [], build: [] });
});

test("parse rejects invalid versions with TypeError", () => {
  for (const bad of ["01.0.0", "1.02.0", "1.0", "v1.0.0", " 1.0.0", "1.0.0-01", "1.0.0-", "1.0.0+", "1.0.0-a..b", 1, null]) {
    assert.throws(() => parse(bad), (error) => error instanceof TypeError && error.message.startsWith("Invalid version"), String(bad));
  }
});

test("build identifiers may have leading zeros", () => {
  assert.deepEqual(parse("1.0.0+007").build, ["007"]);
});

test("core parts compare numerically", () => {
  assert.equal(compare("10.0.0", "9.0.0"), 1);
  assert.equal(compare("1.10.0", "1.9.0"), 1);
  assert.equal(compare("1.0.9", "1.0.10"), -1);
  assert.equal(compare("2.0.0", "2.0.0"), 0);
});

test("the SemVer specification ordering example", () => {
  const ordered = [
    "1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta",
    "1.0.0-beta.2", "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0",
  ];
  for (let i = 0; i < ordered.length - 1; i++) {
    assert.equal(compare(ordered[i], ordered[i + 1]), -1, `${ordered[i]} < ${ordered[i + 1]}`);
    assert.equal(compare(ordered[i + 1], ordered[i]), 1, `${ordered[i + 1]} > ${ordered[i]}`);
  }
});

test("numeric identifiers rank below alphanumeric ones", () => {
  assert.equal(compare("1.0.0-1", "1.0.0-a"), -1);
  assert.equal(compare("1.0.0-a", "1.0.0-1"), 1);
});

test("build metadata is ignored", () => {
  assert.equal(compare("1.0.0+a", "1.0.0+b"), 0);
  assert.equal(compare("1.0.0-rc.1+x", "1.0.0-rc.1"), 0);
});

test("compare rejects invalid input", () => {
  assert.throws(() => compare("1.0", "1.0.0"), TypeError);
});

test("sortVersions returns a new sorted array and keeps ties in order", () => {
  const input = ["1.0.0+b", "1.0.0-rc.1", "0.9.0", "1.0.0+a", "10.0.0"];
  const sorted = sortVersions(input);
  assert.deepEqual(sorted, ["0.9.0", "1.0.0-rc.1", "1.0.0+b", "1.0.0+a", "10.0.0"]);
  assert.deepEqual(input, ["1.0.0+b", "1.0.0-rc.1", "0.9.0", "1.0.0+a", "10.0.0"]);
  assert.notEqual(sorted, input);
});
