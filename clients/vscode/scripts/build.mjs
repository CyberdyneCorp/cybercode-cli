// Bundles the extension host (CommonJS, Node), the webview (IIFE, browser) and, with
// --tests, the unit and integration tests. Nothing is loaded from a network at runtime.
import { build, context } from "esbuild";
import { copyFile, readdir, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const args = new Set(process.argv.slice(2));
const production = args.has("--production");
const sdk = join(root, "../../sdk/typescript/src/index.ts");

const common = {
  bundle: true,
  target: "es2022",
  logLevel: "warning",
  legalComments: "none",
  minify: production,
  sourcemap: production ? false : "inline",
  alias: { "@cyber-code/sdk": sdk },
};

const targets = [
  { ...common, entryPoints: [join(root, "src/extension/extension.ts")], outfile: join(root, "dist/extension.js"), platform: "node", format: "cjs", external: ["vscode"] },
  { ...common, entryPoints: [join(root, "src/webview/main.ts")], outfile: join(root, "dist/webview.js"), platform: "browser", format: "iife" },
];

async function tests() {
  const unit = (await readdir(join(root, "test/unit"))).filter((name) => name.endsWith(".test.ts"));
  const node = { ...common, minify: false, platform: "node", format: "cjs" };
  return [
    { ...node, entryPoints: unit.map((name) => join(root, "test/unit", name)), outdir: join(root, "out/unit"), external: ["vscode"] },
    { ...node, entryPoints: [join(root, "test/integration/run.ts")], outfile: join(root, "out/integration/run.js"), packages: "external" },
    { ...node, entryPoints: [join(root, "test/integration/suite/index.ts")], outfile: join(root, "out/integration/suite/index.js"), external: ["vscode", "mocha"] },
  ];
}

if (args.has("--tests")) {
  await rm(join(root, "out"), { recursive: true, force: true });
  targets.push(...(await tests()));
}
if (production) await copyFile(join(root, "../../LICENSE"), join(root, "LICENSE"));
if (args.has("--watch")) {
  for (const options of targets) await (await context(options)).watch();
} else {
  await Promise.all(targets.map((options) => build(options)));
}
