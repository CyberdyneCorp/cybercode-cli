// Reference copy of the range semantics in SPEC.md section 2, used by the brute-force oracle.
//
// Every term of a range is reduced to a half-open interval [lo, hi) of versions, where a
// missing `hi` means unbounded. An alternative is the intersection of its intervals and a
// range is the union of its alternatives.

const NUMBER = /^(0|[1-9][0-9]*)$/;
const WILDCARDS = new Set(["x", "X", "*"]);
const OPERATORS = [">=", "<=", ">", "<", "=", "^", "~"];
const ZERO = [0, 0, 0];

export function isVersion(text) {
  return typeof text === "string" && text.split(".").length === 3 && text.split(".").every((p) => NUMBER.test(p));
}

export function parseVersion(text) {
  if (!isVersion(text)) throw new Error(`invalid version "${text}"`);
  return text.split(".").map(Number);
}

export function compareVersions(a, b) {
  for (let i = 0; i < 3; i++) {
    if (a[i] !== b[i]) return a[i] < b[i] ? -1 : 1;
  }
  return 0;
}

/** Bump part `index` of `parts` (padded with zeros) and reset the following parts. */
function bump(parts, index) {
  const result = [0, 0, 0];
  for (let i = 0; i < index; i++) result[i] = parts[i];
  result[index] = parts[index] + 1;
  return result;
}

/** Parse a partial like `1.2`, `1.x` or `*` into its numeric parts, or null if invalid. */
function parsePartial(text) {
  const parts = text.split(".");
  if (parts.length > 3) return null;
  const numbers = [];
  let wildcard = false;
  for (const part of parts) {
    if (WILDCARDS.has(part)) wildcard = true;
    else if (NUMBER.test(part) && !wildcard) numbers.push(Number(part));
    else return null;
  }
  return numbers;
}

/** The interval [lo, hi) of versions a partial with these numeric parts denotes. */
function partialInterval(numbers) {
  const lo = [...numbers, 0, 0, 0].slice(0, 3);
  const hi = numbers.length === 0 ? null : bump(lo, numbers.length - 1);
  return { lo, hi };
}

function caretUpper(numbers) {
  if (numbers.length === 0) return null;
  const nonZero = numbers.findIndex((n) => n !== 0);
  return bump(numbers, nonZero === -1 ? numbers.length - 1 : nonZero);
}

function tildeUpper(numbers) {
  if (numbers.length === 0) return null;
  return bump(numbers, numbers.length >= 2 ? 1 : 0);
}

const NOTHING = { lo: ZERO, hi: ZERO };

function termInterval(operator, numbers) {
  const { lo, hi } = partialInterval(numbers);
  switch (operator) {
    case "":
    case "=":
      return { lo, hi };
    case ">":
      return hi === null ? NOTHING : { lo: hi, hi: null };
    case ">=":
      return { lo, hi: null };
    case "<":
      return { lo: ZERO, hi: lo };
    case "<=":
      return { lo: ZERO, hi };
    case "~":
      return { lo, hi: tildeUpper(numbers) };
    default: // "^"
      return { lo, hi: caretUpper(numbers) };
  }
}

function parseTerm(text) {
  const operator = OPERATORS.find((op) => text.startsWith(op)) ?? "";
  const numbers = parsePartial(text.slice(operator.length));
  return numbers === null ? null : termInterval(operator, numbers);
}

function parseAlternative(text) {
  const terms = text.split(" ").filter((t) => t !== "");
  if (terms.length === 0) return null;
  const intervals = terms.map(parseTerm);
  return intervals.includes(null) ? null : intervals;
}

/**
 * Compile a range string into a predicate over parsed versions.
 * Throws `invalid range "<range>"` when the range is not valid.
 */
export function parseRange(range) {
  const alternatives = typeof range === "string" ? range.split("||").map(parseAlternative) : [null];
  if (alternatives.includes(null)) throw new Error(`invalid range "${range}"`);
  return (version) =>
    alternatives.some((intervals) =>
      intervals.every(
        ({ lo, hi }) => compareVersions(version, lo) >= 0 && (hi === null || compareVersions(version, hi) < 0),
      ),
    );
}

export function isValidRange(range) {
  try {
    parseRange(range);
    return true;
  } catch {
    return false;
  }
}

export function satisfies(version, range) {
  const parsed = parseVersion(version);
  return parseRange(range)(parsed);
}
