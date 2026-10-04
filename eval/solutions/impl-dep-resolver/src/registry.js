// Loading and validating registry.json and root.json (SPEC.md sections 3 and 5).
import { compareVersions, isVersion, parseRange, parseVersion } from "./semver.js";

/** An invalid input file; the message is the exact line to print on stderr (exit 2). */
export class InputError extends Error {}

const isObject = (value) => typeof value === "object" && value !== null && !Array.isArray(value);
const byCodeUnit = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

function isDependencyMap(value) {
  return isObject(value) && Object.values(value).every((range) => typeof range === "string");
}

/** Compile `{name: range}` into `[{name, range, test}]` sorted by name; `where` names the owner. */
function compileDependencies(dependencies, where) {
  return Object.keys(dependencies)
    .sort(byCodeUnit)
    .map((name) => {
      const range = dependencies[name];
      try {
        return { name, range, test: parseRange(range) };
      } catch {
        throw new InputError(`error: invalid range "${range}" for dependency ${name} of ${where}`);
      }
    });
}

/** Returns the root's compiled dependencies. */
export function loadRoot(root) {
  if (!isObject(root) || !isDependencyMap(root.dependencies)) throw new InputError("error: invalid root");
  return compileDependencies(root.dependencies, "root");
}

function isRegistryShape(registry) {
  if (!isObject(registry)) return false;
  return Object.values(registry).every(
    (releases) =>
      isObject(releases) &&
      Object.values(releases).every(
        (release) => isObject(release) && (release.dependencies === undefined || isDependencyMap(release.dependencies)),
      ),
  );
}

function loadReleases(name, releases) {
  const loaded = Object.keys(releases)
    .sort(byCodeUnit)
    .map((version) => {
      if (!isVersion(version)) throw new InputError(`error: invalid version "${version}" of ${name}`);
      const dependencies = compileDependencies(releases[version].dependencies ?? {}, `${name}@${version}`);
      return { name, version, parsed: parseVersion(version), dependencies };
    });
  return loaded.sort((a, b) => compareVersions(b.parsed, a.parsed));
}

/** Returns a Map from package name to its releases, highest version first. */
export function loadRegistry(registry) {
  if (!isRegistryShape(registry)) throw new InputError("error: invalid registry");
  const packages = new Map();
  for (const name of Object.keys(registry).sort(byCodeUnit)) {
    packages.set(name, loadReleases(name, registry[name]));
  }
  return packages;
}
