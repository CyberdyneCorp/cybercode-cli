// Optimal dependency resolution (SPEC.md section 4).
//
// Depth-first search over the registry's package names in code-unit order, trying each
// package's versions from highest to lowest and then "absent". Because the options are tried
// in the order of the optimality relation, the first valid complete assignment is the optimal
// one. Partial assignments are pruned as soon as they cannot be completed:
//   - a choice must satisfy every requirement already placed on it by the root and by the
//     present packages before it, and its own dependencies on earlier packages;
//   - a package with requirements must stay satisfiable by at least one of its versions;
//   - a present package must keep a possible parent (the root, or another package that is
//     either undecided or present and depending on it); full reachability is checked at the end.
// Packages that no release reachable from the root can ever depend on are always absent.

const byCodeUnit = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
const ROOT = null;

/** Names reachable from the root through any release, ignoring version constraints. */
function possiblyReachable(registry, rootDependencies) {
  const seen = new Set();
  const queue = rootDependencies.map((d) => d.name);
  while (queue.length > 0) {
    const name = queue.pop();
    if (seen.has(name) || !registry.has(name)) continue;
    seen.add(name);
    for (const release of registry.get(name)) queue.push(...release.dependencies.map((d) => d.name));
  }
  return seen;
}

/** For each name, the other packages with at least one release depending on it. */
function possibleParents(registry) {
  const parents = new Map([...registry.keys()].map((name) => [name, new Set()]));
  for (const [name, releases] of registry) {
    for (const release of releases) {
      for (const dep of release.dependencies) {
        if (dep.name !== name && parents.has(dep.name)) parents.get(dep.name).add(name);
      }
    }
  }
  return parents;
}

class Search {
  constructor(registry, rootDependencies) {
    this.registry = registry;
    this.names = [...registry.keys()].sort(byCodeUnit);
    this.reachable = possiblyReachable(registry, rootDependencies);
    this.parents = possibleParents(registry);
    this.chosen = new Map(); // name -> release, or null for absent
    this.requirements = new Map(this.names.map((name) => [name, []])); // name -> [{from, test}]
    for (const dep of rootDependencies) this.requirements.get(dep.name)?.push({ from: ROOT, test: dep.test });
    this.rootDependencies = rootDependencies;
  }

  run() {
    if (this.rootDependencies.some((dep) => !this.registry.has(dep.name))) return null;
    return this.search(0) ? new Map([...this.chosen].filter(([, release]) => release !== null)) : null;
  }

  options(name) {
    return this.reachable.has(name) ? [...this.registry.get(name), null] : [null];
  }

  search(index) {
    if (index === this.names.length) return this.allReachable();
    const name = this.names[index];
    for (const release of this.options(name)) {
      if (!this.accepts(name, release)) continue;
      this.choose(name, release);
      if (this.stillCompletable(release) && this.search(index + 1)) return true;
      this.unchoose(name, release);
    }
    return false;
  }

  accepts(name, release) {
    const requirements = this.requirements.get(name);
    if (release === null) return requirements.length === 0;
    if (!requirements.every((req) => req.test(release.parsed))) return false;
    return release.dependencies.every((dep) => {
      if (!this.registry.has(dep.name)) return false;
      const target = dep.name === name ? release : this.chosen.get(dep.name);
      return target === undefined || (target !== null && dep.test(target.parsed));
    });
  }

  choose(name, release) {
    this.chosen.set(name, release);
    for (const dep of release?.dependencies ?? []) this.requirements.get(dep.name).push({ from: name, test: dep.test });
  }

  unchoose(name, release) {
    this.chosen.delete(name);
    for (const dep of release?.dependencies ?? []) this.requirements.get(dep.name).pop();
  }

  stillCompletable(release) {
    const undecidedDeps = (release?.dependencies ?? []).filter((dep) => !this.chosen.has(dep.name));
    if (!undecidedDeps.every((dep) => this.canSatisfy(dep.name))) return false;
    return [...this.chosen].every(([name, chosen]) => chosen === null || this.mayHaveParent(name));
  }

  canSatisfy(name) {
    const requirements = this.requirements.get(name);
    return this.registry.get(name).some((release) => requirements.every((req) => req.test(release.parsed)));
  }

  mayHaveParent(name) {
    if (this.requirements.get(name).some((req) => req.from !== name)) return true;
    return [...this.parents.get(name)].some((parent) => !this.chosen.has(parent));
  }

  allReachable() {
    const seen = new Set();
    const queue = this.rootDependencies.map((dep) => dep.name);
    while (queue.length > 0) {
      const name = queue.pop();
      if (seen.has(name)) continue;
      seen.add(name);
      queue.push(...this.chosen.get(name).dependencies.map((dep) => dep.name));
    }
    return [...this.chosen].every(([name, release]) => release === null || seen.has(name));
  }
}

/**
 * Returns the optimal valid resolution as a Map from package name to its chosen release
 * (present packages only), or null when no valid resolution exists.
 */
export function resolve(registry, rootDependencies) {
  return new Search(registry, rootDependencies).run();
}
