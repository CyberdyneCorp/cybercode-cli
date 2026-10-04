// Reading registry.json and root.json.
import { compareVersions, parseVersion } from "./semver.js";

/** An invalid input file; the message is the exact line to print on stderr. */
export class InputError extends Error {}

/** Returns the root's dependencies as `[{name, range}]`. */
export function loadRoot(root) {
  if (!root || typeof root.dependencies !== "object") throw new InputError("error: invalid root");
  return Object.entries(root.dependencies).map(([name, range]) => ({ name, range }));
}

/** Returns a Map from package name to its releases, highest version first. */
export function loadRegistry(registry) {
  if (!registry || typeof registry !== "object") throw new InputError("error: invalid registry");
  const packages = new Map();
  for (const [name, releases] of Object.entries(registry)) {
    const loaded = Object.entries(releases).map(([version, release]) => ({
      name,
      version,
      parsed: parseVersion(version),
      dependencies: Object.entries(release.dependencies ?? {}).map(([dep, range]) => ({ name: dep, range })),
    }));
    loaded.sort((a, b) => compareVersions(b.parsed, a.parsed));
    packages.set(name, loaded);
  }
  return packages;
}
