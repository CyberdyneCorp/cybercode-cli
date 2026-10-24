import { build } from "esbuild";
import { readdir } from "node:fs/promises";
import { spawnSync } from "node:child_process";
const files = (await readdir("test")).filter((name) => name.endsWith(".test.ts")).sort();
if (!files.length) throw new Error("No dashboard regression tests found");
await build({ entryPoints: files.map((name) => `test/${name}`), outdir: ".test-build", bundle: true, platform: "node", format: "esm", target: "node22", packages: "external", logLevel: "warning" });
const run = spawnSync(process.execPath, ["--test", ...files.map((name) => `.test-build/${name.replace(/\.ts$/, ".js")}`)], { stdio: "inherit" });
process.exitCode = run.status ?? 1;
