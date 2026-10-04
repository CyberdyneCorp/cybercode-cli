// Hidden tests for impl-dep-resolver. Every expectation follows SPEC.md:
//   - satisfies() table (sections 1, 2), including exact error messages (2.3);
//   - CLI cases with hand-written expectations: optimality (4), errors and their order (5),
//     lockfile bytes (6);
//   - random small registries checked byte for byte against a brute-force oracle that
//     enumerates every assignment and keeps the greatest valid one (4, 6);
//   - a 67-package registry that must resolve within the time limit stated in the task.
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { satisfies } from "./src/semver.js";
import { satisfies as refSatisfies, parseVersion, compareVersions } from "./hidden_ref_semver.mjs";

// ---------------------------------------------------------------- satisfies() table

const SATISFIES = [
  ["1.2.3", "1.2.3", true], ["1.2.4", "1.2.3", false], ["1.2.3", "=1.2.3", true],
  ["1.2.3", "1.2", true], ["1.3.0", "1.2", false], ["1.9.9", "1", true], ["2.0.0", "1", false],
  ["1.2.0", "=1.2", true], ["0.0.0", "*", true], ["99.0.1", "x", true], ["5.5.5", "X.X.X", true],
  ["1.2.7", "1.2.x", true], ["1.2.7", "1.2.*", true], ["1.3.0", "1.2.X", false],
  ["1.0.0", "1.x.x", true], ["1.99.0", "1.*", true], ["2.0.0", "1.*.*", false],
  ["1.2.4", ">1.2.3", true], ["1.2.3", ">1.2.3", false], ["1.2.9", ">1.2", false],
  ["1.3.0", ">1.2", true], ["1.99.0", ">1", false], ["2.0.0", ">1.x", true], ["1.0.0", ">*", false],
  ["1.2.0", ">=1.2", true], ["1.1.9", ">=1.2", false], ["0.0.0", ">=*", true],
  ["1.1.9", "<1.2", true], ["1.2.0", "<1.2", false], ["0.0.0", "<0.0.0", false], ["0.0.0", "<*", false],
  ["0.9.9", "<1", true], ["1.2.9", "<=1.2", true], ["1.3.0", "<=1.2", false], ["1.2.3", "<=1.2.3", true],
  ["1.2.4", "<=1.2.3", false], ["100.0.0", "<=*", true], ["1.9.9", "<=1.x", true], ["2.0.0", "<=1.x", false],
  ["1.2.3", "~1.2.3", true], ["1.2.2", "~1.2.3", false], ["1.2.99", "~1.2.3", true], ["1.3.0", "~1.2.3", false],
  ["1.2.0", "~1.2", true], ["1.3.0", "~1.2", false], ["1.9.0", "~1", true], ["2.0.0", "~1", false],
  ["0.2.5", "~0.2.3", true], ["0.3.0", "~0.2.3", false], ["1.5.0", "~1.x", true], ["7.0.0", "~*", true],
  ["0.0.9", "~0.0.1", true], ["0.1.0", "~0.0.1", false], ["1.2.5", "~1.2.x", true],
  ["1.2.3", "^1.2.3", true], ["1.99.0", "^1.2.3", true], ["2.0.0", "^1.2.3", false], ["1.2.2", "^1.2.3", false],
  ["0.2.9", "^0.2.3", true], ["0.3.0", "^0.2.3", false], ["0.2.2", "^0.2.3", false],
  ["0.0.3", "^0.0.3", true], ["0.0.4", "^0.0.3", false], ["1.0.0", "^1.2", false], ["1.9.0", "^1.2", true],
  ["0.0.9", "^0.0", true], ["0.1.0", "^0.0", false], ["0.99.0", "^0", true], ["1.0.0", "^0", false],
  ["0.0.5", "^0.0.x", true], ["0.1.0", "^0.0.x", false], ["0.0.0", "^0.0.0", true], ["0.0.1", "^0.0.0", false],
  ["0.5.0", "^0.x", true], ["3.0.0", "^*", true], ["0.2.0", "^0.2", true], ["0.3.0", "^0.2", false],
  ["1.0.0", "^1.x", true], ["2.0.0", "^1.x", false],
  ["1.5.0", ">=1.2.3 <2.0.0", true], ["1.0.0", ">=1.0.0 <1.0.0", false], ["1.5.0", "  >=1.2.3   <2.0.0  ", true],
  ["1.2.3", "1.x || 2.x", true], ["2.5.0", "1.x || 2.x", true], ["3.0.0", "1.x || 2.x", false],
  ["2.0.0", "1.x||2.x", true], ["3.1.0", "<1.0.0 || >=3.0.0 <3.1.0 || 3.1.0", true],
  ["3.0.5", ">=3.0.0 <3.1.0||^5", true], ["5.0.0", "^1 || ~2.1 || >4", true], ["2.2.0", "^1 || ~2.1 || >4", false],
  ["1.10.0", ">1.9.0", true], ["1.9.0", ">=1.10.0", false], ["10.0.0", ">9", true], ["0.10.0", "^0.9", false],
  ["0.9.12", "^0.9", true], ["1.2.3", "1.2.3 1.2.3", true],
  ["1.2.3", "^1.0.0 ~1.2.0 >=1.2.3 <=1.2.3 =1.2.3", true], ["1.2.4", "^1.0.0 ~1.2.0 >=1.2.3 <=1.2.3 =1.2.3", false],
  ["0.0.0", ">=0.0.0", true], ["2.0.0", "<2", false],
];

const INVALID_RANGES = [
  "", "   ", "1.x ||", "|| 1.x", "1.x | 2.x", ">= 1.2.3", ">=", "1.x.3", "*.2", "01.2.3", "1.2.3.4",
  "1.2.3-beta", "v1.2.3", "1.2.3\t", "\t1.2.3", "^~1.2.3", "~>1.2", "1.2.3 - 2.0.0", "=>1.2.3", "1..2",
  ".1", "1.", "x.1", "a", "1.x || || 2.x", "<>1", "1.2.3 ||| 2", "^ 1.2.3", "1.2.3,1.2.4", "==1.2.3",
  "~ 1", "1.x ||  ", "1.2.3+build", "1.x.x.x",
];

const INVALID_VERSIONS = [
  "1.2", "01.2.3", "1.02.3", "1.2.03", "1.2.3-beta", "v1.2.3", " 1.2.3", "1.2.3 ", "1.2.3.4", "", "1.2.x",
  "-1.2.3", "1.2.3+build", "1..3",
];

for (const [version, range, expected] of SATISFIES) {
  test(`satisfies(${JSON.stringify(version)}, ${JSON.stringify(range)}) is ${expected}`, () => {
    assert.equal(satisfies(version, range), expected);
  });
}

for (const range of INVALID_RANGES) {
  test(`satisfies("1.0.0", ${JSON.stringify(range)}) throws invalid range`, () => {
    assert.throws(() => satisfies("1.0.0", range), { message: `invalid range "${range}"` });
  });
}

for (const version of INVALID_VERSIONS) {
  test(`satisfies(${JSON.stringify(version)}, "*") throws invalid version`, () => {
    assert.throws(() => satisfies(version, "*"), { message: `invalid version "${version}"` });
  });
}

test("satisfies checks the version before the range", () => {
  assert.throws(() => satisfies("1.2", ">>1"), { message: 'invalid version "1.2"' });
});

// ---------------------------------------------------------------- CLI helpers

function runCli(args, cwd = process.cwd()) {
  const result = spawnSync(process.execPath, [join(process.cwd(), "src/cli.js"), ...args], {
    cwd,
    encoding: "utf8",
    timeout: 20000,
  });
  return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}

function writeInputs(registry, root) {
  const dir = mkdtempSync(join(tmpdir(), "pinpack-"));
  for (const [name, value] of [["registry.json", registry], ["root.json", root]]) {
    if (value !== undefined) writeFileSync(join(dir, name), typeof value === "string" ? value : JSON.stringify(value));
  }
  return dir;
}

function resolveWith(registry, root) {
  const dir = writeInputs(registry, root);
  return runCli(["resolve", "registry.json", "root.json"], dir);
}

/** The exact lockfile text for `{name: [version, {dep: version}]}`. */
function lockfileText(packages) {
  const sorted = {};
  for (const name of Object.keys(packages).sort()) {
    const [version, deps] = packages[name];
    const dependencies = {};
    for (const dep of Object.keys(deps).sort()) dependencies[dep] = deps[dep];
    sorted[name] = { version, dependencies };
  }
  return JSON.stringify({ lockfileVersion: 1, packages: sorted }, null, 2) + "\n";
}

function assertSuccess(result, expectedText) {
  assert.deepEqual(result, { status: 0, stdout: expectedText, stderr: "" });
}

function assertFailure(result, status, message) {
  assert.deepEqual(result, { status, stdout: "", stderr: `${message}\n` });
}

const NO_RESOLUTION = "error: no resolution satisfies the root dependencies";
const rel = (deps = {}) => ({ dependencies: deps });

// ---------------------------------------------------------------- CLI: resolution semantics

const RESOLUTION_CASES = [
  {
    title: "spec example: presence of an earlier package beats a higher later version",
    registry: { c: { "2.0.0": {}, "1.0.0": rel({ a: "*" }) }, a: { "1.0.0": {} } },
    root: { dependencies: { c: "*" } },
    expected: { a: ["1.0.0", {}], c: ["1.0.0", { a: "1.0.0" }] },
  },
  {
    title: "optimality walks names in code-unit order (B before a)",
    registry: { a: { "2.0.0": rel({ B: "1.x" }), "1.0.0": rel({ B: "2.x" }) }, B: { "1.0.0": {}, "2.0.0": {} } },
    root: { dependencies: { a: "*", B: "*" } },
    expected: { B: ["2.0.0", {}], a: ["1.0.0", { B: "2.0.0" }] },
  },
  {
    title: "optimality walks names in code-unit order (a10 before a9)",
    registry: { a9: { "2.0.0": rel({ a10: "^1" }), "1.0.0": rel({ a10: "^2" }) }, a10: { "1.0.0": {}, "2.0.0": {} } },
    root: { dependencies: { a9: "*", a10: "*" } },
    expected: { a10: ["2.0.0", {}], a9: ["1.0.0", { a10: "2.0.0" }] },
  },
  {
    title: "versions compare numerically",
    registry: { x: { "1.9.0": {}, "1.10.0": {}, "1.2.0": {} } },
    root: { dependencies: { x: "^1" } },
    expected: { x: ["1.10.0", {}] },
  },
  {
    title: "backtracks over a choice two levels up",
    registry: {
      app: { "2.0.0": rel({ lib: "^2", util: "^1" }), "1.0.0": rel({ lib: "*" }) },
      lib: { "2.0.0": rel({ util: "^2" }), "2.1.0": rel({ util: ">=2" }) },
      util: { "1.0.0": {}, "2.0.0": {} },
    },
    root: { dependencies: { app: "*" } },
    expected: { app: ["1.0.0", { lib: "2.1.0" }], lib: ["2.1.0", { util: "2.0.0" }], util: ["2.0.0", {}] },
  },
  {
    title: "a release depending on a missing package is never chosen",
    registry: { a: { "2.0.0": rel({ ghost: "*" }), "1.0.0": {} } },
    root: { dependencies: { a: "*" } },
    expected: { a: ["1.0.0", {}] },
  },
  {
    title: "self-dependencies are checked like any other",
    registry: { a: { "2.0.0": rel({ a: "^1" }), "1.5.0": rel({ a: "^1" }) } },
    root: { dependencies: { a: "*" } },
    expected: { a: ["1.5.0", { a: "1.5.0" }] },
  },
  {
    title: "cycles reachable from the root are resolved",
    registry: { a: { "1.0.0": rel({ b: "*" }) }, b: { "1.0.0": rel({ a: "^1" }), "2.0.0": rel({ a: "^2" }) } },
    root: { dependencies: { a: "*" } },
    expected: { a: ["1.0.0", { b: "1.0.0" }], b: ["1.0.0", { a: "1.0.0" }] },
  },
  {
    title: "a cycle nothing chosen depends on stays absent",
    registry: { a: { "2.0.0": {}, "1.0.0": rel({ b: "*" }) }, b: { "1.0.0": rel({ c: "*" }) }, c: { "1.0.0": rel({ b: "*" }) } },
    root: { dependencies: { a: "*" } },
    expected: { a: ["2.0.0", {}] },
  },
  {
    title: "packages unreachable from the root are left out",
    registry: { a: { "1.0.0": {} }, b: { "1.0.0": rel({ a: "*" }) } },
    root: { dependencies: { a: "*" } },
    expected: { a: ["1.0.0", {}] },
  },
  {
    title: "lockfile dependency keys are sorted",
    registry: { a: { "1.0.0": rel({ z: "*", m: "*", B: "*" }) }, z: { "1.0.0": {} }, m: { "1.0.0": {} }, B: { "1.0.0": {} } },
    root: { dependencies: { a: "*" } },
    expected: { a: ["1.0.0", { B: "1.0.0", m: "1.0.0", z: "1.0.0" }], B: ["1.0.0", {}], m: ["1.0.0", {}], z: ["1.0.0", {}] },
  },
  {
    title: "ranges with alternatives in the registry",
    registry: { a: { "1.0.0": rel({ b: "1.x || 3.x" }) }, b: { "1.0.0": {}, "2.0.0": {}, "3.0.0": {}, "4.0.0": {} } },
    root: { dependencies: { a: "*" } },
    expected: { a: ["1.0.0", { b: "3.0.0" }], b: ["3.0.0", {}] },
  },
  {
    title: "caret on 0.x versions limits the minor version",
    registry: { a: { "0.2.3": {}, "0.2.9": {}, "0.3.0": {} } },
    root: { dependencies: { a: "^0.2.3" } },
    expected: { a: ["0.2.9", {}] },
  },
  {
    title: "root ranges and registry ranges intersect",
    registry: { a: { "1.0.0": rel({ b: ">=1.2 <1.4" }) }, b: { "1.1.0": {}, "1.2.0": {}, "1.3.5": {}, "1.4.0": {} } },
    root: { dependencies: { a: "*", b: "~1.2 || 1.4.0" } },
    expected: { a: ["1.0.0", { b: "1.2.0" }], b: ["1.2.0", {}] },
  },
  {
    title: "a release object without dependencies has none",
    registry: { a: { "1.0.0": { description: "ignored" } } },
    root: { name: "app", dependencies: { a: "1.0.0" } },
    expected: { a: ["1.0.0", {}] },
  },
];

for (const { title, registry, root, expected } of RESOLUTION_CASES) {
  test(`cli: ${title}`, () => {
    assertSuccess(resolveWith(registry, root), lockfileText(expected));
  });
}

test("cli: empty root dependencies give an empty lockfile", () => {
  assertSuccess(resolveWith({ a: { "1.0.0": {} } }, { dependencies: {} }), '{\n  "lockfileVersion": 1,\n  "packages": {}\n}\n');
});

test("cli: a root dependency on a missing package has no resolution", () => {
  assertFailure(resolveWith({ a: { "1.0.0": {} } }, { dependencies: { a: "*", ghost: "*" } }), 1, NO_RESOLUTION);
});

test("cli: conflicting requirements have no resolution", () => {
  const registry = { a: { "1.0.0": rel({ c: "^1" }) }, b: { "1.0.0": rel({ c: "^2" }) }, c: { "1.0.0": {}, "2.0.0": {} } };
  assertFailure(resolveWith(registry, { dependencies: { a: "^1", b: "^1" } }), 1, NO_RESOLUTION);
});

test("cli: a range matching no published version has no resolution", () => {
  assertFailure(resolveWith({ a: { "1.0.0": {} } }, { dependencies: { a: ">=1.0.0 <1.0.0" } }), 1, NO_RESOLUTION);
});

// ---------------------------------------------------------------- CLI: errors and their order

const USAGE = "usage: node src/cli.js resolve <registry.json> <root.json>";

test("cli: no arguments print the usage line", () => {
  assertFailure(runCli([]), 2, USAGE);
});

test("cli: an unknown command prints the usage line", () => {
  const dir = writeInputs({}, { dependencies: {} });
  assertFailure(runCli(["install", "registry.json", "root.json"], dir), 2, USAGE);
});

test("cli: an extra argument prints the usage line", () => {
  const dir = writeInputs({}, { dependencies: {} });
  assertFailure(runCli(["resolve", "registry.json", "root.json", "extra"], dir), 2, USAGE);
});

test("cli: a missing registry file cannot be read", () => {
  const dir = writeInputs(undefined, { dependencies: {} });
  assertFailure(runCli(["resolve", "./nope/registry.json", "root.json"], dir), 2, "error: cannot read ./nope/registry.json");
});

test("cli: the registry file is read before the root file", () => {
  const dir = writeInputs("{not json", undefined);
  assertFailure(runCli(["resolve", "registry.json", "root.json"], dir), 2, "error: cannot read registry.json");
});

test("cli: a root file that is not JSON cannot be read", () => {
  assertFailure(resolveWith({}, "{\"dependencies\": {"), 2, "error: cannot read root.json");
});

const ERROR_CASES = [
  ["a root that is an array is invalid", {}, [], "error: invalid root"],
  ["a root without dependencies is invalid", {}, { name: "app" }, "error: invalid root"],
  ["root dependencies must be an object", {}, { dependencies: null }, "error: invalid root"],
  ["root ranges must be strings", {}, { dependencies: { a: 1 } }, "error: invalid root"],
  [
    "root ranges are checked in code-unit name order",
    { a: { "1.0.0": {} } },
    { dependencies: { b: "^1", a: "1.x.3", B: ">= 1" } },
    'error: invalid range ">= 1" for dependency B of root',
  ],
  ["root ranges are checked before the registry", [], { dependencies: { a: "^^1" } }, 'error: invalid range "^^1" for dependency a of root'],
  ["a registry that is an array is invalid", [], { dependencies: {} }, "error: invalid registry"],
  ["a registry that is null is invalid", null, { dependencies: {} }, "error: invalid registry"],
  ["a package that is not an object is invalid", { a: ["1.0.0"] }, { dependencies: {} }, "error: invalid registry"],
  ["a release that is null is invalid", { a: { "1.0.0": null } }, { dependencies: {} }, "error: invalid registry"],
  ["release dependencies must be an object", { a: { "1.0.0": { dependencies: ["b"] } } }, { dependencies: {} }, "error: invalid registry"],
  [
    "registry structure is checked before version keys",
    { a: { "01.0.0": {} }, b: { "1.0.0": rel({ c: 5 }) } },
    { dependencies: {} },
    "error: invalid registry",
  ],
  ["an invalid version key is reported", { b: { "1.0.0": {} }, a: { "1.0": {} } }, { dependencies: { b: "*" } }, 'error: invalid version "1.0" of a'],
  [
    "version keys are checked in code-unit string order",
    { a: { "2.0.0": rel({ b: "~>1" }), "10.0.0": rel({ b: "^x.1" }) }, b: { "1.0.0": {} } },
    { dependencies: {} },
    'error: invalid range "^x.1" for dependency b of a@10.0.0',
  ],
  [
    "a version key is checked before its ranges",
    { a: { "1.0.0": rel({ b: "bad" }), "1.0": {} } },
    { dependencies: {} },
    'error: invalid version "1.0" of a',
  ],
  [
    "dependency ranges are checked in code-unit name order",
    { a: { "1.0.0": rel({ z: "!", b: "?" }) } },
    { dependencies: {} },
    'error: invalid range "?" for dependency b of a@1.0.0',
  ],
  [
    "packages are checked in code-unit name order",
    { b: { "1.0.0": rel({ x: "1.2.3.4" }) }, Z: { "1.0.0": rel({ x: "1.x.1" }) } },
    { dependencies: {} },
    'error: invalid range "1.x.1" for dependency x of Z@1.0.0',
  ],
  [
    "unreachable packages are validated too",
    { a: { "1.0.0": {} }, zz: { "1.0.0": rel({ a: "1.2.3.4" }) } },
    { dependencies: { a: "*" } },
    'error: invalid range "1.2.3.4" for dependency a of zz@1.0.0',
  ],
];

for (const [title, registry, root, message] of ERROR_CASES) {
  test(`cli: ${title}`, () => {
    assertFailure(resolveWith(registry, root), 2, message);
  });
}

// ---------------------------------------------------------------- random registries vs brute force

function mulberry32(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const NAME_POOL = ["a", "b", "c", "B", "a10", "a9", "dep-x", "Zeta", "é", "lib"];
const VERSION_POOL = ["0.0.1", "0.0.2", "0.1.0", "0.1.5", "0.2.0", "1.0.0", "1.2.0", "1.9.0", "1.10.0", "2.0.0", "2.1.3", "3.0.0"];
const OPS = ["", "=", ">", ">=", "<", "<=", "^", "~", "^", "~", "^", ">="];

function randomCase(seed) {
  const rand = mulberry32(seed);
  const pick = (items) => items[Math.floor(rand() * items.length)];
  const shuffled = (items) => {
    const copy = [...items];
    for (let i = copy.length - 1; i > 0; i--) {
      const j = Math.floor(rand() * (i + 1));
      [copy[i], copy[j]] = [copy[j], copy[i]];
    }
    return copy;
  };
  const names = shuffled(NAME_POOL).slice(0, 3 + Math.floor(rand() * 4));
  const registry = {};
  for (const name of names) {
    registry[name] = {};
    for (const version of shuffled(VERSION_POOL).slice(0, 1 + Math.floor(rand() * 4))) registry[name][version] = {};
  }
  // Ranges are built around versions the target actually has, so most of them can match.
  const range = (target) => {
    const versions = registry[target] && rand() < 0.95 ? Object.keys(registry[target]) : VERSION_POOL;
    const term = () => {
      if (rand() < 0.06) return "*";
      const parts = pick(versions).split(".");
      const keep = 1 + Math.floor(rand() * 3);
      const shown = parts.slice(0, keep);
      if (keep < 3 && rand() < 0.3) shown.push("x");
      return pick(OPS) + shown.join(".");
    };
    const alternative = () => (rand() < 0.12 ? `${term()} ${term()}` : term());
    return rand() < 0.25 ? `${alternative()}${pick([" || ", "||"])}${alternative()}` : alternative();
  };
  const target = () => (rand() < 0.04 ? "ghost" : pick(names));
  for (const name of names) {
    for (const version of Object.keys(registry[name])) {
      if (rand() < 0.15) continue; // release object without a dependencies key
      const dependencies = {};
      const count = pick([0, 1, 1, 2, 2, 3]);
      for (let j = 0; j < count; j++) {
        const dep = target();
        dependencies[dep] = range(dep);
      }
      registry[name][version] = { dependencies };
    }
  }
  const rootDependencies = {};
  const rootCount = pick([1, 2, 2, 3]);
  for (let j = 0; j < rootCount; j++) {
    const dep = rand() < 0.01 ? "ghost" : pick(names);
    rootDependencies[dep] = range(dep);
  }
  return { registry, root: { dependencies: rootDependencies } };
}

/** Greatest valid resolution by exhaustive enumeration (SPEC.md section 4), or null. */
function bruteForce(registry, rootDeps) {
  const names = Object.keys(registry).sort();
  const options = names.map((name) => [null, ...Object.keys(registry[name])]);
  const depsOf = (name, version) => registry[name][version].dependencies ?? {};
  const isValid = (choice) => {
    const ok = (deps) =>
      Object.entries(deps).every(([dep, range]) => typeof choice[dep] === "string" && refSatisfies(choice[dep], range));
    if (!ok(rootDeps)) return false;
    if (!names.every((name) => choice[name] === null || ok(depsOf(name, choice[name])))) return false;
    const seen = new Set();
    const queue = Object.keys(rootDeps);
    while (queue.length > 0) {
      const name = queue.pop();
      if (seen.has(name)) continue;
      seen.add(name);
      queue.push(...Object.keys(depsOf(name, choice[name])));
    }
    return names.every((name) => choice[name] === null || seen.has(name));
  };
  const greater = (a, b) => {
    for (const name of names) {
      if (a[name] === b[name]) continue;
      if (b[name] === null) return true;
      if (a[name] === null) return false;
      return compareVersions(parseVersion(a[name]), parseVersion(b[name])) > 0;
    }
    return false;
  };
  let best = null;
  const index = names.map(() => 0);
  for (;;) {
    const choice = Object.fromEntries(names.map((name, i) => [name, options[i][index[i]]]));
    if (isValid(choice) && (best === null || greater(choice, best))) best = choice;
    let i = 0;
    while (i < names.length && ++index[i] === options[i].length) index[i++] = 0;
    if (i === names.length) break;
  }
  if (best === null) return null;
  const packages = {};
  for (const name of names) {
    if (best[name] === null) continue;
    const deps = Object.fromEntries(Object.keys(depsOf(name, best[name])).map((dep) => [dep, best[dep]]));
    packages[name] = [best[name], deps];
  }
  return packages;
}

// Seeds picked from 1..1500 to mix unsatisfiable registries, registries where including an
// earlier package outranks a later package's higher version, and ordinary ones.
const RANDOM_SEEDS = [
  1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 29,
  30, 31, 35, 38, 41, 42, 43, 44, 45, 55, 56, 59, 60, 62, 63, 65, 66, 67, 68, 69, 70, 71, 73,
  74, 75, 76, 77, 78, 81, 86, 88, 90, 91, 92, 93, 96, 98, 103, 104, 110, 137, 139, 156, 162,
  200, 208, 225, 227, 229, 292, 399, 416, 426, 428, 429, 462, 488, 505, 506, 510, 523, 548, 551,
  605, 621, 638, 684, 756, 808, 821, 862, 867, 879, 882,
];
for (const seed of RANDOM_SEEDS) {
  test(`cli: random registry #${seed} matches the brute-force optimum`, () => {
    const { registry, root } = randomCase(seed);
    const expected = bruteForce(registry, root.dependencies);
    const result = resolveWith(registry, root);
    if (expected === null) assertFailure(result, 1, NO_RESOLUTION);
    else assertSuccess(result, lockfileText(expected));
  });
}

// ---------------------------------------------------------------- performance

test("cli: resolves the 67-package registry within 2 seconds", () => {
  const registry = readFileSync(join(process.cwd(), "hidden_perf_registry.json"), "utf8");
  const root = readFileSync(join(process.cwd(), "hidden_perf_root.json"), "utf8");
  const expected = readFileSync(join(process.cwd(), "hidden_perf_expected.json"), "utf8");
  const started = performance.now();
  const result = resolveWith(registry, root);
  const elapsed = performance.now() - started;
  assertSuccess(result, expected);
  assert.ok(elapsed < 2000, `took ${Math.round(elapsed)} ms`);
});
