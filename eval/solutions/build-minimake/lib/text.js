'use strict';
// Small text helpers shared by the expander, the functions and the parser.

const words = (text) => text.split(/[ \t\n]+/).filter(Boolean);

const strip = (text) => text.replace(/^[ \t\n]+|[ \t\n]+$/g, '');

/** Match `word` against a pattern whose first `%` matches any string; returns the stem or null. */
function matchPercent(pattern, word) {
  const at = pattern.indexOf('%');
  if (at < 0) return word === pattern ? '' : null;
  const prefix = pattern.slice(0, at);
  const suffix = pattern.slice(at + 1);
  if (word.length < prefix.length + suffix.length) return null;
  if (!word.startsWith(prefix) || !word.endsWith(suffix)) return null;
  return word.slice(prefix.length, word.length - suffix.length);
}

/** Replace the first `%` of `pattern` with `stem` (no `%`: the pattern itself). */
const fillPercent = (pattern, stem) => pattern.replace('%', () => stem);

/** Index of the character closing the reference opened at `open` (`(` or `{`), or -1. */
function findClose(text, open) {
  const opener = text[open];
  const closer = opener === '(' ? ')' : '}';
  let depth = 0;
  for (let i = open; i < text.length; i++) {
    if (text[i] === opener) depth++;
    else if (text[i] === closer && --depth === 0) return i;
  }
  return -1;
}

module.exports = { words, strip, matchPercent, fillPercent, findClose };
