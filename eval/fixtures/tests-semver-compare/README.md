# semver

`src/semver.js` (Node 22, ES modules, no dependencies) implements version parsing and
precedence for a subset of [SemVer 2.0.0](https://semver.org). The code is correct but untested.
Tests go under `test/` as `*.test.js` files using `node:test` and `node:assert`, run with
`node --test`.

## Contract

`parse(version)` returns `{ major, minor, patch, prerelease, build }`:

- `version` is `MAJOR.MINOR.PATCH`, optionally followed by `-PRERELEASE` and then `+BUILD`.
  `major`, `minor`, `patch` are numbers; `prerelease` and `build` are arrays of the
  dot-separated identifier strings (empty arrays when absent).
- MAJOR, MINOR, PATCH and numeric prerelease identifiers must not have leading zeros (`0` is
  fine, `01` is not). Identifiers are non-empty and use only `[0-9A-Za-z-]`.
- Anything else (including a `v` prefix, surrounding whitespace or a non-string) throws a
  `TypeError` whose message starts with `Invalid version`.

`compare(a, b)` takes two version strings and returns `-1`, `0` or `1`:

- MAJOR, then MINOR, then PATCH are compared numerically.
- A version with a prerelease has lower precedence than the same version without one.
- Prerelease identifiers are compared left to right: numeric identifiers numerically,
  alphanumeric identifiers lexically in ASCII order, and numeric identifiers always lower than
  alphanumeric ones. If all shared identifiers are equal, the shorter list is lower.
- Build metadata is ignored, so `1.0.0+a` and `1.0.0+b` compare equal.
- Invalid input throws like `parse`.

`sortVersions(versions)` returns a **new** array sorted ascending by `compare`; versions of
equal precedence keep their input order. The input array is not modified.
