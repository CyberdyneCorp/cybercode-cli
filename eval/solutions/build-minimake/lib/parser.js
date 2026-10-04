'use strict';
// Reads a makefile into variables, explicit rules and pattern rules (SPEC §2).
const { MakeError } = require('./errors');
const { err } = require('./output');
const { findClose, strip, words } = require('./text');

const DIRECTIVES = new Set(['ifeq', 'ifneq', 'ifdef', 'ifndef', 'else', 'endif']);

/** First top-level `=` or `:` of a makefile line: { kind: 'rule' | ':=' | '?=' | '+=' | '=', at }. */
function findSeparator(line) {
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (c === '$') {
      if (line[i + 1] === '(' || line[i + 1] === '{') {
        const close = findClose(line, i + 1);
        if (close < 0) return null;
        i = close;
      } else i++;
    } else if (c === ':') {
      return line[i + 1] === '=' ? { kind: ':=', at: i } : { kind: 'rule', at: i };
    } else if (c === '=') {
      const before = line[i - 1];
      return before === '?' || before === '+' ? { kind: `${before}=`, at: i - 1 } : { kind: '=', at: i };
    }
  }
  return null;
}

/** Split `(A,B)` into [A, B], or null when the text has another shape. */
function conditionalArguments(text) {
  const rest = text.replace(/^[ \t]+/, '');
  if (!rest.startsWith('(')) return null;
  let depth = 0;
  let comma = -1;
  for (let i = 0; i < rest.length; i++) {
    const c = rest[i];
    if (c === '(') depth++;
    else if (c === ',' && depth === 1 && comma < 0) comma = i;
    else if (c === ')' && --depth === 0) {
      if (comma < 0 || strip(rest.slice(i + 1)) !== '') return null;
      return [rest.slice(1, comma), rest.slice(comma + 1, i)];
    }
  }
  return null;
}

class Makefile {
  constructor() {
    this.rules = new Map(); // target -> { prereqs, orderOnly, recipe | null }
    this.patternRules = []; // { target, prereqs, orderOnly, recipe }
    this.phony = new Set();
    this.defaultGoal = null;
  }
}

class Parser {
  constructor(fileName, text, vars, expander) {
    this.fileName = fileName;
    this.lines = text.split('\n');
    if (this.lines.at(-1) === '') this.lines.pop();
    this.vars = vars;
    this.expander = expander;
    this.makefile = new Makefile();
    this.conditionals = []; // { active, parentActive, taken, sawElse, line }
    this.recipeContext = null; // { recipe, targets, line, attached }
  }

  error(lineNo, message) {
    return new MakeError(`${this.fileName}:${lineNo}: *** ${message}.  Stop.`);
  }

  get active() {
    return this.conditionals.length === 0 || this.conditionals.at(-1).active;
  }

  parse() {
    for (let i = 0; i < this.lines.length; i++) {
      const lineNo = i + 1;
      if (this.lines[i].startsWith('\t') && strip(this.lines[i]) !== '') {
        i = this.recipeLine(i, lineNo);
        continue;
      }
      let line = this.lines[i];
      while (line.endsWith('\\') && i + 1 < this.lines.length) {
        line = `${line.slice(0, -1).replace(/[ \t]+$/, '')} ${this.lines[++i].replace(/^[ \t]+/, '')}`;
      }
      if (line.endsWith('\\')) line = line.slice(0, -1);
      const hash = line.indexOf('#');
      if (hash >= 0) line = line.slice(0, hash);
      if (strip(line) !== '') this.makefileLine(line, lineNo);
    }
    if (this.conditionals.length > 0) throw this.error(this.conditionals.at(-1).line, "missing 'endif'");
    return this.makefile;
  }

  /** Handle the recipe line starting at index i; returns the index of its last physical line. */
  recipeLine(i, lineNo) {
    let text = this.lines[i].slice(1);
    while (text.endsWith('\\') && i + 1 < this.lines.length) {
      text += `\n${this.lines[++i].replace(/^\t/, '')}`;
    }
    if (!this.active) return i;
    if (!this.recipeContext) throw this.error(lineNo, 'recipe commences before first target');
    this.attachRecipe(this.recipeContext);
    this.recipeContext.recipe.push(text);
    return i;
  }

  attachRecipe(context) {
    if (context.attached) return;
    context.attached = true;
    for (const target of context.targets) {
      const rule = this.makefile.rules.get(target);
      if (rule.recipe) {
        err(`minimake: ${this.fileName}:${context.line}: warning: overriding recipe for target '${target}'`);
      }
      rule.recipe = context.recipe;
    }
  }

  makefileLine(line, lineNo) {
    const first = words(line)[0];
    if (DIRECTIVES.has(first)) {
      this.directive(first, line.slice(line.indexOf(first) + first.length), lineNo);
      return;
    }
    if (!this.active) return;
    const separator = findSeparator(line);
    if (!separator) {
      if (strip(this.expander.expand(line)) !== '') throw this.error(lineNo, 'missing separator');
      return;
    }
    if (separator.kind === 'rule') {
      this.rule(line.slice(0, separator.at), line.slice(separator.at + 1), lineNo);
      return;
    }
    this.recipeContext = null;
    const name = strip(this.expander.expand(line.slice(0, separator.at)));
    const value = strip(line.slice(separator.at + separator.kind.length));
    this.vars.assign(name, separator.kind, value, (text) => this.expander.expand(text));
  }

  directive(keyword, rest, lineNo) {
    const top = this.conditionals.at(-1);
    if (keyword === 'else') {
      if (!top || top.sawElse) throw this.error(lineNo, "extraneous 'else'");
      top.sawElse = true;
      top.active = top.parentActive && !top.taken;
      return;
    }
    if (keyword === 'endif') {
      if (!top) throw this.error(lineNo, "extraneous 'endif'");
      this.conditionals.pop();
      return;
    }
    const parentActive = this.active;
    const taken = parentActive && this.evaluate(keyword, rest, lineNo);
    this.conditionals.push({ active: taken, parentActive, taken, sawElse: false, line: lineNo });
  }

  evaluate(keyword, rest, lineNo) {
    if (keyword === 'ifdef' || keyword === 'ifndef') {
      const variable = this.vars.lookup(strip(this.expander.expand(rest)));
      return (variable !== undefined && variable.value !== '') === (keyword === 'ifdef');
    }
    const args = conditionalArguments(rest);
    if (!args) throw this.error(lineNo, 'invalid syntax in conditional');
    const [a, b] = args.map((arg) => strip(this.expander.expand(arg)));
    return (a === b) === (keyword === 'ifeq');
  }

  rule(targetText, prereqText, lineNo) {
    const targets = words(this.expander.expand(targetText));
    const all = words(this.expander.expand(prereqText));
    const bar = all.indexOf('|');
    const prereqs = bar < 0 ? all : all.slice(0, bar);
    const orderOnly = bar < 0 ? [] : all.slice(bar + 1).filter((p) => p !== '|');
    const context = { recipe: [], targets: [], line: lineNo, attached: false };
    this.recipeContext = context;
    if (targets.length === 0) return;
    if (targets[0].includes('%')) {
      context.attached = true;
      this.makefile.patternRules.push({ target: targets[0], prereqs, orderOnly, recipe: context.recipe });
      return;
    }
    for (const target of targets) {
      if (target === '.PHONY') {
        for (const name of [...prereqs, ...orderOnly]) this.makefile.phony.add(name);
        continue;
      }
      if (this.makefile.defaultGoal === null && !target.startsWith('.') && !target.includes('%')) {
        this.makefile.defaultGoal = target;
      }
      if (!this.makefile.rules.has(target)) this.makefile.rules.set(target, { prereqs: [], orderOnly: [], recipe: null });
      const rule = this.makefile.rules.get(target);
      rule.prereqs.push(...prereqs);
      rule.orderOnly.push(...orderOnly);
      context.targets.push(target);
    }
  }
}

function parseMakefile(fileName, text, vars, expander) {
  return new Parser(fileName, text, vars, expander).parse();
}

module.exports = { parseMakefile };
