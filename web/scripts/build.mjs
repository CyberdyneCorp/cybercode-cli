import { build } from "esbuild";
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
const root = dirname(dirname(fileURLToPath(import.meta.url)));
const artifacts = new Map([["index.html", await readFile(join(root, "index.html"))]]);
for (const [input, output] of [["main.ts", "app.js"], ["app.css", "app.css"]]) {
  const result = await build({ entryPoints: [join(root, "src", input)], outfile: join(root, "dist", output), bundle: true, platform: "browser", format: "esm", target: "es2022", minify: true, legalComments: "none", write: false, logLevel: "warning" });
  artifacts.set(output, result.outputFiles[0].contents);
}
if (process.argv.includes("--check")) {
  for (const [name, bytes] of artifacts) {
    const current = await readFile(join(root, "dist", name)).catch(() => Buffer.alloc(0));
    if (!current.equals(Buffer.from(bytes))) { process.stderr.write(`web/dist/${name} is stale; run npm run build in web\n`); process.exitCode = 1; }
  }
} else {
  await mkdir(join(root, "dist"), { recursive: true });
  for (const [name, bytes] of artifacts) await writeFile(join(root, "dist", name), bytes);
}
