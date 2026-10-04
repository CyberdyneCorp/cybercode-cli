#!/usr/bin/env node
// pinpack: `node src/cli.js resolve <registry.json> <root.json>` (SPEC.md section 5).
import { readFileSync } from "node:fs";

import { formatLockfile } from "./lockfile.js";
import { InputError, loadRegistry, loadRoot } from "./registry.js";
import { resolve } from "./resolver.js";

const USAGE = "usage: node src/cli.js resolve <registry.json> <root.json>";

class CliError extends Error {
  constructor(message, code) {
    super(message);
    this.code = code;
  }
}

function readJson(path) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch {
    throw new CliError(`error: cannot read ${path}`, 2);
  }
}

function run(args) {
  if (args.length !== 3 || args[0] !== "resolve") throw new CliError(USAGE, 2);
  const registryJson = readJson(args[1]);
  const rootJson = readJson(args[2]);
  const rootDependencies = loadRoot(rootJson);
  const registry = loadRegistry(registryJson);
  const resolution = resolve(registry, rootDependencies);
  if (resolution === null) throw new CliError("error: no resolution satisfies the root dependencies", 1);
  return formatLockfile(resolution);
}

try {
  process.stdout.write(run(process.argv.slice(2)));
} catch (error) {
  if (error instanceof InputError) error.code = 2;
  else if (!(error instanceof CliError)) throw error;
  process.stderr.write(`${error.message}\n`);
  process.exitCode = error.code;
}
