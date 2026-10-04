// Lockfile rendering.

/** Render the lockfile for a resolution: a Map from package name to its chosen release. */
export function formatLockfile(resolution) {
  const packages = {};
  for (const name of [...resolution.keys()].sort()) {
    const release = resolution.get(name);
    const dependencies = {};
    for (const dep of release.dependencies) dependencies[dep.name] = resolution.get(dep.name).version;
    packages[name] = { version: release.version, dependencies };
  }
  return JSON.stringify({ lockfileVersion: 1, packages }, null, 2) + "\n";
}
