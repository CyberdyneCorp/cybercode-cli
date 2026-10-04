'use strict';
// Built-in functions (SPEC §4.3). Each entry: [argument count, implementation].
// Implementations receive the expander and the raw (unexpanded) arguments.
const { spawnSync } = require('node:child_process');
const { MakeError } = require('./errors');
const { out } = require('./output');
const { glob } = require('./glob');
const { words, strip, matchPercent, fillPercent } = require('./text');

const UNLIMITED = Infinity;

const compare = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
const lastSlash = (name) => name.lastIndexOf('/');

function suffixStart(name) {
  const dot = name.lastIndexOf('.');
  return dot > lastSlash(name) ? dot : -1;
}

function patsubst(pattern, replacement, text) {
  return words(text).map((word) => {
    const stem = matchPercent(pattern, word);
    return stem === null ? word : fillPercent(replacement, stem);
  }).join(' ');
}

const matchesAny = (patterns, word) => patterns.some((p) => matchPercent(p, word) !== null);

function word(nText, text) {
  const n = strip(nText);
  if (!/^[0-9]+$/.test(n)) throw new MakeError(`*** non-numeric first argument to 'word' function: '${n}'.  Stop.`);
  if (Number(n) === 0) throw new MakeError("*** first argument to 'word' function must be greater than 0.  Stop.");
  return words(text)[Number(n) - 1] ?? '';
}

function join(a, b) {
  const left = words(a);
  const right = words(b);
  const length = Math.max(left.length, right.length);
  return Array.from({ length }, (_, i) => (left[i] ?? '') + (right[i] ?? '')).join(' ');
}

function shell(command) {
  const result = spawnSync('/bin/sh', ['-c', command], { stdio: ['inherit', 'pipe', 'inherit'], encoding: 'utf8' });
  return (result.stdout ?? '').replace(/\n+$/, '').replace(/\n/g, ' ');
}

/** Functions whose arguments are all expanded before they run. */
const eager = {
  subst: [3, (from, to, text) => (from === '' ? text : text.split(from).join(to))],
  patsubst: [3, (pattern, replacement, text) => patsubst(strip(pattern), strip(replacement), text)],
  strip: [1, (text) => words(text).join(' ')],
  findstring: [2, (find, text) => (text.includes(find) ? find : '')],
  filter: [2, (patterns, text) => words(text).filter((w) => matchesAny(words(patterns), w)).join(' ')],
  'filter-out': [2, (patterns, text) => words(text).filter((w) => !matchesAny(words(patterns), w)).join(' ')],
  sort: [1, (list) => [...new Set(words(list))].sort(compare).join(' ')],
  word: [2, word],
  words: [1, (text) => String(words(text).length)],
  firstword: [1, (text) => words(text)[0] ?? ''],
  lastword: [1, (text) => words(text).at(-1) ?? ''],
  dir: [1, (names) => words(names).map((n) => (lastSlash(n) < 0 ? './' : n.slice(0, lastSlash(n) + 1))).join(' ')],
  notdir: [1, (names) => words(names).map((n) => n.slice(lastSlash(n) + 1)).join(' ')],
  suffix: [1, (names) => words(names).filter((n) => suffixStart(n) >= 0).map((n) => n.slice(suffixStart(n))).join(' ')],
  basename: [1, (names) => words(names).map((n) => (suffixStart(n) < 0 ? n : n.slice(0, suffixStart(n)))).join(' ')],
  addprefix: [2, (prefix, names) => words(names).map((n) => prefix + n).join(' ')],
  addsuffix: [2, (suffix, names) => words(names).map((n) => n + suffix).join(' ')],
  join: [2, join],
  wildcard: [1, (patterns) => words(patterns).flatMap(glob).join(' ')],
  shell: [1, shell],
  info: [1, (text) => { out(text); return ''; }],
  error: [1, (text) => { throw new MakeError(`*** ${text}.  Stop.`); }],
};

/** Functions that control the expansion of their own arguments. */
const lazy = {
  foreach: [3, (x, name, list, text) => {
    const variable = strip(x.expand(name));
    return words(x.expand(list))
      .map((item) => x.vars.withScope(new Map([[variable, item]]), false, () => x.expand(text)))
      .join(' ');
  }],
  if: [3, (x, condition, then, otherwise = '') => (strip(x.expand(condition)) !== '' ? x.expand(then) : x.expand(otherwise))],
  call: [UNLIMITED, (x, name, ...args) => {
    const variable = strip(x.expand(name));
    const bindings = new Map([['0', variable], ...args.map((a, i) => [String(i + 1), x.expand(a)])]);
    const definition = x.vars.lookup(variable);
    if (!definition) return '';
    return x.vars.withScope(bindings, true, () => x.expand(definition.value));
  }],
  flavor: [1, (x, name) => x.vars.lookup(strip(x.expand(name)))?.flavor ?? 'undefined'],
};

module.exports = { eager, lazy };
