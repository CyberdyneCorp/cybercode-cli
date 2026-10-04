'use strict';
// Variable storage and lookup order (SPEC §3, §4.2).

const AUTOMATIC = new Set(['@', '<', '^', '+', '?', '*', '@D', '@F', '<D', '<F']);

class Variables {
  constructor() {
    this.file = new Map(); // name -> { flavor: 'recursive' | 'simple', value }
    this.command = new Map(); // name -> unexpanded value (always recursive)
    this.scopes = []; // foreach/call bindings, innermost last: { values: Map, digitsEmpty }
    this.automatic = null; // Map while a recipe is being expanded
  }

  defineCommandLine(name, value) {
    this.command.set(name, value);
  }

  /** Apply a makefile assignment. `expand` expands text immediately (for := and simple +=). */
  assign(name, op, value, expand) {
    if (this.command.has(name)) return;
    const old = this.file.get(name);
    if (op === '?=' && old) return;
    if (op === ':=') {
      this.file.set(name, { flavor: 'simple', value: expand(value) });
    } else if (op === '+=' && old) {
      const added = old.flavor === 'simple' ? expand(value) : value;
      old.value = old.value === '' ? added : `${old.value} ${added}`;
    } else {
      this.file.set(name, { flavor: 'recursive', value });
    }
  }

  /** The variable as { flavor, value }, or undefined. Bindings and automatics are simple. */
  lookup(name) {
    for (let i = this.scopes.length - 1; i >= 0; i--) {
      const scope = this.scopes[i];
      if (scope.values.has(name)) return { flavor: 'simple', value: scope.values.get(name) };
      if (scope.digitsEmpty && /^[0-9]+$/.test(name)) return { flavor: 'simple', value: '' };
    }
    if (AUTOMATIC.has(name)) {
      return this.automatic ? { flavor: 'simple', value: this.automatic.get(name) ?? '' } : undefined;
    }
    if (this.command.has(name)) return { flavor: 'recursive', value: this.command.get(name) };
    return this.file.get(name);
  }

  /** Run `fn` with extra simple bindings in an inner scope. */
  withScope(values, digitsEmpty, fn) {
    this.scopes.push({ values, digitsEmpty });
    try {
      return fn();
    } finally {
      this.scopes.pop();
    }
  }
}

module.exports = { Variables };
