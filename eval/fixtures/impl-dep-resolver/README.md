# pinpack

A tiny package-manager resolver (Node 22, ES modules, no dependencies). Given a registry of
published releases and a project's root dependencies, it picks one version of each needed
package and prints a lockfile:

```sh
node src/cli.js resolve test/fixtures/registry.json test/fixtures/root.json
```

| Module | Role |
|---|---|
| `src/cli.js` | Command line: argument handling, error messages, exit codes |
| `src/registry.js` | Reads and validates `registry.json` and `root.json` |
| `src/semver.js` | Versions and ranges; exports `satisfies(version, range)` |
| `src/resolver.js` | Chooses the versions |
| `src/lockfile.js` | Renders the lockfile |

`SPEC.md` is the authoritative contract. The current code is a first draft: the range parser
only knows exact versions and simple comparators, and the resolver greedily takes the highest
matching version of each package, so it fails on conflicts a backtracking search would solve.

Run the tests with `node --test`.
