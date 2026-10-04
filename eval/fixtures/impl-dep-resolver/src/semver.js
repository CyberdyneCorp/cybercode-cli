// Versions and ranges. Draft: only exact versions, comparators and caret are understood.

export function parseVersion(text) {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(text);
  if (!match) throw new Error(`invalid version "${text}"`);
  return match.slice(1).map(Number);
}

export function compareVersions(a, b) {
  for (let i = 0; i < 3; i++) {
    if (a[i] !== b[i]) return a[i] - b[i];
  }
  return 0;
}

function matchesTerm(version, term) {
  const match = /^(>=|<=|>|<|=|\^)?(.*)$/.exec(term);
  const operator = match[1] ?? "=";
  const target = parseVersion(match[2]);
  const cmp = compareVersions(version, target);
  switch (operator) {
    case ">=":
      return cmp >= 0;
    case "<=":
      return cmp <= 0;
    case ">":
      return cmp > 0;
    case "<":
      return cmp < 0;
    case "^":
      return cmp >= 0 && version[0] === target[0];
    default:
      return cmp === 0;
  }
}

export function satisfies(version, range) {
  const parsed = parseVersion(version);
  const terms = range.trim().split(/\s+/);
  try {
    return terms.every((term) => matchesTerm(parsed, term));
  } catch {
    throw new Error(`invalid range "${range}"`);
  }
}
