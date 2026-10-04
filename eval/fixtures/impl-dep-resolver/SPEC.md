# pinpack resolver specification

This file is the authoritative contract. Node 22, ES modules, no dependencies.

## 1. Versions

A version is `MAJOR.MINOR.PATCH`: exactly three dot-separated non-negative decimal integers,
each either `0` or a digit `1-9` followed by digits (no leading zeros, no sign, no `v` prefix,
no pre-release or build suffix, no surrounding whitespace). `1.2.3` and `0.10.0` are versions;
`1.2`, `01.2.3`, `1.2.3-beta`, `v1.2.3` and ` 1.2.3` are not.

Versions are ordered numerically part by part: MAJOR first, then MINOR, then PATCH
(`1.10.0` > `1.9.5` > `1.9.0` > `0.99.99`).

## 2. Ranges

### 2.1 Grammar

The only whitespace character is the ASCII space (U+0020); any other character not in the
grammar (tab, `-`, `v`, letters other than `x`/`X`, ...) makes the range invalid.

```
range        := alternative ( "||" alternative )*
alternative  := space* term ( space+ term )* space*
term         := operator? partial
operator     := ">=" | "<=" | ">" | "<" | "=" | "^" | "~"
partial      := part | part "." part | part "." part "." part
part         := number | "x" | "X" | "*"
number       := "0" | [1-9] [0-9]*
```

- `x`, `X` and `*` are wildcards. Once a part is a wildcard, every following part must be a
  wildcard too: `1.x.x` and `1.*` are valid, `1.x.3` and `*.2` are not.
- An operator is written directly before its partial: `>=1.2.3` is one term, `>= 1.2.3` is
  invalid (the term `>=` has no partial).
- Every alternative contains at least one term: `""`, `"  "`, `"1.x ||"` and `"|| 1.x"` are
  invalid. `||` needs no surrounding spaces (`1.x||2.x` is valid); a single `|` is invalid.

### 2.2 Meaning

Terms separated by spaces within an alternative are ANDed. Alternatives separated by `||` are
ORed. A version satisfies a range when it satisfies every term of at least one alternative.

A partial `P` (the numeric parts given before any wildcard) denotes a half-open interval of
versions `[lo, hi)`:

| Partial | lo | hi (exclusive) |
|---|---|---|
| `M.m.p` | `M.m.p` | `M.m.(p+1)` |
| `M.m`, `M.m.x` | `M.m.0` | `M.(m+1).0` |
| `M`, `M.x`, `M.x.x` | `M.0.0` | `(M+1).0.0` |
| `*`, `x`, `x.x.x`, ... | `0.0.0` | none (unbounded) |

A term with partial `P = [lo, hi)` matches the versions `v` with:

| Term | Matches |
|---|---|
| `P`, `=P` | `lo <= v < hi` |
| `>P` | `v >= hi` (nothing when `hi` is unbounded) |
| `>=P` | `v >= lo` |
| `<P` | `v < lo` (nothing when `lo` is `0.0.0`) |
| `<=P` | `v < hi` (every version when `hi` is unbounded) |
| `~P` | `lo <= v < T` where `T` = `M.(m+1).0` if at least MAJOR and MINOR are numeric, `(M+1).0.0` if only MAJOR is numeric; every version if `P` is a wildcard |
| `^P` | `lo <= v < C` where `C` is found from the numeric parts of `P`: take the first numeric part that is not `0`, or the last numeric part if all are `0`, add one to it and set every part after it to `0`; every version if `P` is a wildcard |

Examples: `>1.2` is `>=1.3.0`; `<=1.2` is `<1.3.0`; `>1.2.3` matches `1.2.4`;
`~1.2.3` is `>=1.2.3 <1.3.0`; `~1.2` is `>=1.2.0 <1.3.0`; `~1` is `>=1.0.0 <2.0.0`;
`^1.2.3` is `>=1.2.3 <2.0.0`; `^0.2.3` is `>=0.2.3 <0.3.0`; `^0.0.3` is `>=0.0.3 <0.0.4`;
`^1.2` is `>=1.2.0 <2.0.0`; `^0.0` is `>=0.0.0 <0.1.0`; `^0` is `>=0.0.0 <1.0.0`;
`^0.0.x` is `>=0.0.0 <0.1.0`; `1.2.*` is `>=1.2.0 <1.3.0`; `*` matches everything;
`>=1.0.0 <1.0.0` matches nothing (and is still a valid range).

### 2.3 `satisfies(version, range)`

`src/semver.js` exports `satisfies(version, range)`, taking two strings and returning a boolean.
It throws an `Error` whose message is exactly `invalid version "<version>"` if `version` is not
a version (section 1), else exactly `invalid range "<range>"` if `range` is not a valid range
(`<version>` and `<range>` are the arguments as given). The version is checked first.

## 3. Input files

`registry.json` lists every published release:

```json
{
  "left-pad": {
    "1.0.0": { "dependencies": {} },
    "1.1.0": { "dependencies": { "core": "^2.0.0" } }
  },
  "core": {
    "2.0.0": {}
  }
}
```

It is a JSON object mapping package names (non-empty strings; names that are canonical array
indices such as `"7"`, and the name `__proto__`, are outside this contract) to objects mapping versions to release
objects. A release object may have a `dependencies` key, an object mapping package names to
range strings; a missing `dependencies` key means no dependencies; other keys are ignored.

`root.json` is the project being installed: an object with a `dependencies` key holding an
object mapping package names to range strings (other keys are ignored).

Structural rules: the registry must be an object (not an array, not null), each package value
an object, each release value an object, each `dependencies` value (when present) an object, and
every range a string. The root must be an object whose `dependencies` key is present and is an
object of strings. "Object" always means a JSON object, never an array or null.

## 4. Resolution

A resolution assigns each package name of the registry either one of its versions (the
package is *present*) or *absent*. It is **valid** when:

1. for every root dependency `D: R`, `D` is present and its version satisfies `R`;
2. for every present package `P@V` and every dependency `D: R` of release `P@V`, `D` is present
   and its version satisfies `R`;
3. every present package is reachable: following edges from the root to each root dependency
   and from each present `P@V` to each dependency of that release, every present package can be
   reached from the root. (A package depending on itself, or a cycle of packages that nothing
   else depends on, is not reachable that way.)

Consequences: only one version of each package is ever chosen; a dependency on a name that is
not in the registry can never be satisfied, so a release with such a dependency is never
chosen (and a root dependency on such a name makes resolution impossible); self-dependencies
and cycles are allowed and are checked like any other dependency.

**Optimality.** Let `N` be the list of all package names in the registry sorted in ascending
UTF-16 code-unit order (what JavaScript's default `Array.prototype.sort` gives, so `"B"` sorts
before `"a"` and `"a10"` before `"a9"`). Two resolutions are compared by walking `N` in order;
at the first name where they differ, the resolution that gives that name the higher version
wins, and any version beats absent. The resolver must output the greatest valid resolution in
this order; it is unique.

Example: the root depends on `c: "*"`; `c@2.0.0` has no dependencies; `c@1.0.0` depends on
`a: "*"`; `a@1.0.0` has no dependencies. Both `{a: absent, c: 2.0.0}` and
`{a: 1.0.0, c: 1.0.0}` are valid; `N = [a, c]` and the second resolution wins at `a`, so the
output is `a@1.0.0, c@1.0.0`.

## 5. Command line

```
node src/cli.js resolve <registry.json> <root.json>
```

On success it writes the lockfile to stdout, writes nothing to stderr and exits 0. Otherwise it
writes exactly one line `<message>\n` to stderr, nothing to stdout, and exits with the code
below. Checks happen in this order, and the first failure wins:

| # | Check | Message | Exit |
|---|---|---|---|
| 1 | exactly three arguments, the first `resolve` | `usage: node src/cli.js resolve <registry.json> <root.json>` | 2 |
| 2 | the registry file can be read and parsed as JSON | `error: cannot read <path>` | 2 |
| 3 | the root file can be read and parsed as JSON | `error: cannot read <path>` | 2 |
| 4 | the root is structurally valid (section 3) | `error: invalid root` | 2 |
| 5 | every root range is valid, checking dependency names in ascending code-unit order | `error: invalid range "<range>" for dependency <name> of root` | 2 |
| 6 | the registry is structurally valid (section 3) | `error: invalid registry` | 2 |
| 7 | every version key and range in the registry is valid: packages in ascending code-unit name order; within a package, version keys in ascending code-unit string order; for each key, first the key itself, then its dependency ranges in ascending code-unit name order | `error: invalid version "<version>" of <package>` or `error: invalid range "<range>" for dependency <name> of <package>@<version>` | 2 |
| 8 | a valid resolution exists | `error: no resolution satisfies the root dependencies` | 1 |

`<path>` is the argument exactly as given. Validation covers the whole registry, including
packages that are not reachable from the root.

## 6. Lockfile

The lockfile lists exactly the present packages of the optimal resolution:

```json
{
  "lockfileVersion": 1,
  "packages": {
    "core": {
      "version": "2.0.0",
      "dependencies": {}
    },
    "left-pad": {
      "version": "1.1.0",
      "dependencies": {
        "core": "2.0.0"
      }
    }
  }
}
```

- Top-level keys are `lockfileVersion` (the number `1`) then `packages`.
- `packages` maps each present package name to `{"version", "dependencies"}` in that key order,
  where `dependencies` maps each dependency name of the chosen release to the version chosen
  for that dependency (the resolved version, not the range).
- The keys of `packages` and of every `dependencies` object are sorted in ascending code-unit
  order.
- The text is exactly what `JSON.stringify(lockfile, null, 2)` produces for that object, followed
  by one `\n`. With no root dependencies the output is
  `{\n  "lockfileVersion": 1,\n  "packages": {}\n}\n`.
