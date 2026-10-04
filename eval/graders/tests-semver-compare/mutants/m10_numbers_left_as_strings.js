// Mutant: parse returns MAJOR as a string
/** SemVer 2.0.0 parsing and precedence. See README.md for the contract. */

const NUMERIC = /^(0|[1-9]\d*)$/;
const IDENTIFIER = /^[0-9A-Za-z-]+$/;
const VERSION = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([^+]+))?(?:\+(.+))?$/;

function invalid(version) {
  return new TypeError(`Invalid version: ${JSON.stringify(version)}`);
}

function identifiers(text, version, { numericNoLeadingZero }) {
  if (text === undefined) return [];
  const parts = text.split(".");
  for (const part of parts) {
    if (!IDENTIFIER.test(part)) throw invalid(version);
    if (numericNoLeadingZero && /^\d+$/.test(part) && !NUMERIC.test(part)) throw invalid(version);
  }
  return parts;
}

export function parse(version) {
  const match = typeof version === "string" ? VERSION.exec(version) : null;
  if (!match) throw invalid(version);
  const [, major, minor, patch, prerelease, build] = match;
  return {
    major,
    minor: Number(minor),
    patch: Number(patch),
    prerelease: identifiers(prerelease, version, { numericNoLeadingZero: true }),
    build: identifiers(build, version, { numericNoLeadingZero: false }),
  };
}

function compareIdentifiers(a, b) {
  const aNumeric = NUMERIC.test(a);
  const bNumeric = NUMERIC.test(b);
  if (aNumeric && bNumeric) return Math.sign(Number(a) - Number(b));
  if (aNumeric !== bNumeric) return aNumeric ? -1 : 1;
  return a < b ? -1 : a > b ? 1 : 0;
}

function comparePrerelease(a, b) {
  if (a.length === 0 || b.length === 0) return Math.sign(b.length - a.length);
  for (let i = 0; i < Math.min(a.length, b.length); i++) {
    const order = compareIdentifiers(a[i], b[i]);
    if (order !== 0) return order;
  }
  return Math.sign(a.length - b.length);
}

export function compare(a, b) {
  const left = parse(a);
  const right = parse(b);
  for (const key of ["major", "minor", "patch"]) {
    if (left[key] !== right[key]) return left[key] < right[key] ? -1 : 1;
  }
  return comparePrerelease(left.prerelease, right.prerelease);
}

export function sortVersions(versions) {
  return versions.toSorted(compare);
}
