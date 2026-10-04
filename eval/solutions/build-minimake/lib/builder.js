'use strict';
// Rule selection, rebuild decisions and recipe execution (SPEC §5-§8).
const fs = require('node:fs');
const os = require('node:os');
const { spawnSync } = require('node:child_process');
const { MakeError } = require('./errors');
const { out, err } = require('./output');
const { matchPercent, fillPercent } = require('./text');

const unique = (list) => [...new Set(list)];

function mtime(name) {
  try {
    return fs.statSync(name).mtimeMs;
  } catch {
    return null;
  }
}

/** Match a pattern-rule target against a name (SPEC §5): { stem, core, dir } or null. */
function matchTarget(pattern, name) {
  const slash = name.lastIndexOf('/');
  const dir = !pattern.includes('/') && slash >= 0 ? name.slice(0, slash + 1) : '';
  const core = matchPercent(pattern, name.slice(dir.length));
  return core ? { stem: dir + core, core, dir } : null;
}

function exitStatus(result) {
  if (result.status !== null) return result.status;
  return 128 + (os.constants.signals[result.signal] ?? 0);
}

class Builder {
  constructor(makefile, vars, expander, options) {
    this.makefile = makefile;
    this.vars = vars;
    this.expander = expander;
    this.options = options; // { dryRun, keepGoing, alwaysMake }
    this.results = new Map(); // target -> { updated, failed, hasRecipe }
    this.inProgress = new Set();
    this.linesRun = 0;
    this.failed = false;
  }

  isPhony(name) {
    return this.makefile.phony.has(name);
  }

  exists(name) {
    return !this.isPhony(name) && mtime(name) !== null;
  }

  /** Update every goal in order; returns the exit status. */
  run(goals) {
    for (const goal of goals) {
      const before = this.linesRun;
      const result = this.update(goal, null);
      if (result.failed) {
        err(`minimake: Target '${goal}' not remade because of errors.`);
      } else if (this.linesRun === before) {
        out(result.hasRecipe ? `minimake: '${goal}' is up to date.` : `minimake: Nothing to be done for '${goal}'.`);
      }
    }
    return this.failed ? 2 : 0;
  }

  /** The rule used for `name`: { recipe, prereqs, orderOnly, stem }, or null if there is none. */
  resolve(name) {
    const explicit = this.makefile.rules.get(name);
    if (explicit?.recipe) return { ...explicit, stem: '' };
    const pattern = this.isPhony(name) ? null : this.findPatternRule(name);
    if (pattern) {
      return {
        recipe: pattern.recipe,
        prereqs: [...pattern.prereqs, ...(explicit?.prereqs ?? [])],
        orderOnly: [...pattern.orderOnly, ...(explicit?.orderOnly ?? [])],
        stem: pattern.stem,
      };
    }
    if (explicit) return { ...explicit, recipe: null, stem: '' };
    if (this.isPhony(name)) return { recipe: null, prereqs: [], orderOnly: [], stem: '' };
    return null;
  }

  findPatternRule(name) {
    let best = null;
    for (const rule of this.makefile.patternRules) {
      if (rule.recipe.length === 0) continue;
      const match = matchTarget(rule.target, name);
      if (!match || (best && best.stem.length <= match.stem.length)) continue;
      const instantiate = (p) => (p.includes('%') ? match.dir + fillPercent(p, match.core) : p);
      const candidate = {
        recipe: rule.recipe,
        prereqs: rule.prereqs.map(instantiate),
        orderOnly: rule.orderOnly.map(instantiate),
        stem: match.stem,
      };
      const qualifies = (p) => this.makefile.rules.has(p) || mtime(p) !== null;
      if ([...candidate.prereqs, ...candidate.orderOnly].every(qualifies)) best = candidate;
    }
    return best;
  }

  fail(message) {
    this.failed = true;
    if (!this.options.keepGoing) throw new MakeError(`${message}.  Stop.`);
    err(`minimake: ${message}.`);
  }

  update(name, dependent) {
    if (this.results.has(name)) return this.results.get(name);
    const result = this.compute(name, dependent);
    this.results.set(name, result);
    return result;
  }

  compute(name, dependent) {
    const rule = this.resolve(name);
    if (!rule) {
      if (mtime(name) !== null) return { updated: false, failed: false, hasRecipe: false };
      const neededBy = dependent === null ? '' : `, needed by '${dependent}'`;
      this.fail(`*** No rule to make target '${name}'${neededBy}`);
      return { updated: false, failed: true, hasRecipe: false };
    }
    const hasRecipe = rule.recipe !== null;
    this.inProgress.add(name);
    const { prereqs, orderOnly, failed } = this.updatePrerequisites(name, rule);
    this.inProgress.delete(name);
    if (failed) return { updated: false, failed: true, hasRecipe };

    const targetTime = this.exists(name) ? mtime(name) : null;
    const isNewer = (p) => this.results.get(p).updated || (this.exists(p) && mtime(p) > targetTime);
    const outOfDate = this.isPhony(name) || this.options.alwaysMake || targetTime === null || prereqs.some(isNewer);
    if (!outOfDate) return { updated: false, failed: false, hasRecipe };
    if (hasRecipe) {
      const newer = targetTime === null ? unique(prereqs) : unique(prereqs.filter(isNewer));
      const automatic = this.automaticVariables(name, rule.stem, prereqs, newer);
      if (!this.runRecipe(name, rule.recipe, automatic)) return { updated: false, failed: true, hasRecipe };
    }
    return { updated: true, failed: false, hasRecipe };
  }

  /** Update prerequisites in order, dropping circular ones (SPEC §6 step 3). */
  updatePrerequisites(name, rule) {
    const normal = unique(rule.prereqs);
    const orderOnly = unique(rule.orderOnly).filter((p) => !normal.includes(p));
    const dropped = new Set();
    let failed = false;
    for (const prereq of [...normal, ...orderOnly]) {
      if (this.inProgress.has(prereq)) {
        err(`minimake: Circular ${name} <- ${prereq} dependency dropped.`);
        dropped.add(prereq);
      } else if (this.update(prereq, name).failed) {
        failed = true;
      }
    }
    const kept = (p) => !dropped.has(p);
    return { prereqs: rule.prereqs.filter(kept), orderOnly: orderOnly.filter(kept), failed };
  }

  automaticVariables(name, stem, prereqs, newer) {
    const first = prereqs[0] ?? '';
    const dirPart = (p) => (p.includes('/') ? p.slice(0, p.lastIndexOf('/')) : '.');
    const filePart = (p) => p.slice(p.lastIndexOf('/') + 1);
    return new Map([
      ['@', name], ['<', first], ['^', unique(prereqs).join(' ')], ['+', prereqs.join(' ')],
      ['?', newer.join(' ')], ['*', stem],
      ['@D', dirPart(name)], ['@F', filePart(name)], ['<D', first && dirPart(first)], ['<F', filePart(first)],
    ]);
  }

  /** Expand and run a recipe; returns false when it failed. */
  runRecipe(name, recipe, automatic) {
    this.vars.automatic = automatic;
    let lines;
    try {
      lines = recipe.map((line) => this.expander.expand(line));
    } finally {
      this.vars.automatic = null;
    }
    for (const line of lines) {
      const prefix = /^[ \t@+-]*/.exec(line)[0];
      const command = line.slice(prefix.length);
      if (command === '') continue;
      this.linesRun++;
      const silent = prefix.includes('@');
      const force = prefix.includes('+');
      if (this.options.dryRun && !force) {
        out(command);
        continue;
      }
      if (!silent) out(command);
      const status = exitStatus(spawnSync('/bin/sh', ['-c', command], { stdio: 'inherit' }));
      if (status === 0) continue;
      if (prefix.includes('-')) {
        err(`minimake: [${name}] Error ${status} (ignored)`);
        continue;
      }
      this.failed = true;
      if (!this.options.keepGoing) throw new MakeError(`*** [${name}] Error ${status}`);
      err(`minimake: *** [${name}] Error ${status}`);
      return false;
    }
    return true;
  }
}

module.exports = { Builder };
