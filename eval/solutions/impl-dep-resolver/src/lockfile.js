// Lockfile rendering (SPEC.md section 6).

const byCodeUnit = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

/** Render the lockfile for a resolution: a Map from package name to its chosen release. */
export function formatLockfile(resolution) {
  const packages = {};
  for (const name of [...resolution.keys()].sort(byCodeUnit)) {
    const release = resolution.get(name);
    const dependencies = {};
    for (const { name: dep } of [...release.dependencies].sort((a, b) => byCodeUnit(a.name, b.name))) {
      dependencies[dep] = resolution.get(dep).version;
    }
    packages[name] = { version: release.version, dependencies };
  }
  return JSON.stringify({ lockfileVersion: 1, packages }, null, 2) + "\n";
}
