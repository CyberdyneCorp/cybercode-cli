// Draft resolver: walks the dependency graph breadth-first and takes the highest version that
// satisfies the first range seen for each package. It never revisits a choice.
import { satisfies } from "./semver.js";

/**
 * Returns a Map from package name to its chosen release, or null when the draft gets stuck.
 */
export function resolve(registry, rootDependencies) {
  const chosen = new Map();
  const queue = [...rootDependencies];
  while (queue.length > 0) {
    const { name, range } = queue.shift();
    const releases = registry.get(name) ?? [];
    if (chosen.has(name)) {
      if (!satisfies(chosen.get(name).version, range)) return null;
      continue;
    }
    const release = releases.find((r) => satisfies(r.version, range));
    if (!release) return null;
    chosen.set(name, release);
    queue.push(...release.dependencies);
  }
  return chosen;
}
