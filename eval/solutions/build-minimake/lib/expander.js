'use strict';
// Text expansion: $$, $(NAME), ${NAME}, $X, substitution references and function calls (SPEC §4).
const { MakeError } = require('./errors');
const { eager, lazy } = require('./functions');
const { findClose, matchPercent, words } = require('./text');

/** Split function arguments at top-level commas; the last argument keeps any extra commas. */
function splitArguments(text, max) {
  const args = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < text.length && args.length < max - 1; i++) {
    const c = text[i];
    if (c === '(' || c === '{') depth++;
    else if ((c === ')' || c === '}') && depth > 0) depth--;
    else if (c === ',' && depth === 0) {
      args.push(text.slice(start, i));
      start = i + 1;
    }
  }
  args.push(text.slice(start));
  while (args.length < max && Number.isFinite(max)) args.push('');
  return args;
}

function substitutionReference(value, from, to) {
  if (from.includes('%')) {
    return words(value).map((w) => {
      const stem = matchPercent(from, w);
      return stem === null ? w : to.replace('%', () => stem);
    }).join(' ');
  }
  return words(value).map((w) => (w.endsWith(from) ? w.slice(0, w.length - from.length) + to : w)).join(' ');
}

class Expander {
  constructor(vars) {
    this.vars = vars;
    this.inExpansion = new Set(); // recursive variables currently being expanded
  }

  expand(text) {
    if (!text.includes('$')) return text;
    let result = '';
    let i = 0;
    while (i < text.length) {
      const dollar = text.indexOf('$', i);
      if (dollar < 0) {
        result += text.slice(i);
        break;
      }
      result += text.slice(i, dollar);
      const next = text[dollar + 1];
      if (next === undefined) break;
      if (next === '(' || next === '{') {
        const close = findClose(text, dollar + 1);
        if (close < 0) throw new MakeError('*** unterminated variable reference.  Stop.');
        result += this.reference(text.slice(dollar + 2, close));
        i = close + 1;
      } else {
        result += next === '$' ? '$' : this.value(next);
        i = dollar + 2;
      }
    }
    return result;
  }

  /** The body of a `$(...)` / `${...}` reference. */
  reference(body) {
    const call = /^([a-z-]+)[ \t]/.exec(body);
    if (call && (call[1] in eager || call[1] in lazy)) {
      return this.callFunction(call[1], body.slice(call[0].length).replace(/^[ \t]+/, ''));
    }
    const name = this.expand(body);
    const colon = name.indexOf(':');
    const equals = colon < 0 ? -1 : name.indexOf('=', colon + 1);
    if (equals < 0) return this.value(name);
    const value = this.value(name.slice(0, colon));
    return substitutionReference(value, name.slice(colon + 1, equals), name.slice(equals + 1));
  }

  callFunction(name, argumentText) {
    if (name in lazy) {
      const [count, fn] = lazy[name];
      return fn(this, ...splitArguments(argumentText, count));
    }
    const [count, fn] = eager[name];
    return fn(...splitArguments(argumentText, count).map((a) => this.expand(a)));
  }

  /** The expanded value of variable `name` (empty when undefined). */
  value(name) {
    const variable = this.vars.lookup(name);
    if (!variable) return '';
    if (variable.flavor === 'simple') return variable.value;
    if (this.inExpansion.has(name)) {
      throw new MakeError(`*** Recursive variable '${name}' references itself (eventually).  Stop.`);
    }
    this.inExpansion.add(name);
    try {
      return this.expand(variable.value);
    } finally {
      this.inExpansion.delete(name);
    }
  }
}

module.exports = { Expander };
