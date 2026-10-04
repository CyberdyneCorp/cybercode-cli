#!/usr/bin/env node
// pinpack: `node src/cli.js resolve <registry.json> <root.json>`.
import { readFileSync } from "node:fs";

import { formatLockfile } from "./lockfile.js";
import { InputError, loadRegistry, loadRoot } from "./registry.js";
import { resolve } from "./resolver.js";

function fail(message, code) {
  process.stderr.write(`${message}\n`);
  process.exit(code);
}

const [command, registryPath, rootPath] = process.argv.slice(2);
if (command !== "resolve") fail("usage: node src/cli.js resolve <registry.json> <root.json>", 2);

try {
  const registry = loadRegistry(JSON.parse(readFileSync(registryPath, "utf8")));
  const root = loadRoot(JSON.parse(readFileSync(rootPath, "utf8")));
  const resolution = resolve(registry, root);
  if (resolution === null) fail("error: no resolution satisfies the root dependencies", 1);
  process.stdout.write(formatLockfile(resolution));
} catch (error) {
  if (error instanceof InputError) fail(error.message, 2);
  fail(`error: ${error.message}`, 2);
}
