// Trusted test bootstrap. Do not load any fixture/package code before apply().
const fs = require("node:fs");
require("node:child_process");
require("node:worker_threads");
const [addonPath, profile, fixture, input] = process.argv.slice(2);
const native = require(addonPath);
if (profile !== "-") native.apply(fs.readFileSync(profile, "utf8"));
globalThis.nativeProof = { native, input: JSON.parse(input) };
require(fixture);
