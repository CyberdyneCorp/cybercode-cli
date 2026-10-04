'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { project } = require('./helpers');

const info = (t, makefile, ...args) => {
  const p = project(t, { Makefile: `${makefile}\nall:\n` });
  const result = p.run(...args);
  assert.equal(result.err, '');
  return result.out.replace("minimake: Nothing to be done for 'all'.\n", '');
};

test('continuations and comments', (t) => {
  assert.equal(info(t, 'X = a   \\\n   b # c \\\n d\n$(info [$(X)])'), '[a b]\n');
});

test('recipe continuation keeps backslash-newline', (t) => {
  const p = project(t, { Makefile: 'x:\n\techo a \\\n\tb\n' });
  assert.equal(p.run().out, 'echo a \\\nb\na b\n');
});

test('parse errors carry file and line', (t) => {
  const p = project(t, { Makefile: 'X = 1\nbad\n' });
  assert.deepEqual(p.run(), { out: '', err: 'minimake: Makefile:2: *** missing separator.  Stop.\n', code: 2 });
  p.write('Makefile', '\techo\n');
  assert.equal(p.run().err, 'minimake: Makefile:1: *** recipe commences before first target.  Stop.\n');
});

test('conditionals', (t) => {
  assert.equal(info(t, 'X = 1\nifeq ($(X), 1 )\nA = y\nelse\nA = n\nendif\nifdef U\nB = y\nelse\nB = n\nendif\n$(info $(A)$(B))'), 'yn\n');
  assert.equal(info(t, 'ifdef X\n$(info set)\nendif', 'X=1'), 'set\n');
});

test('conditional errors', (t) => {
  const p = project(t, { Makefile: 'ifdef A\n' });
  assert.equal(p.run().err, "minimake: Makefile:1: *** missing 'endif'.  Stop.\n");
  p.write('Makefile', 'endif\n');
  assert.equal(p.run().err, "minimake: Makefile:1: *** extraneous 'endif'.  Stop.\n");
  p.write('Makefile', 'ifeq (a)\nendif\n');
  assert.equal(p.run().err, 'minimake: Makefile:1: *** invalid syntax in conditional.  Stop.\n');
});

test('overriding a recipe warns', (t) => {
  const p = project(t, { Makefile: 'x:\n\t@echo 1\nx:\n\t@echo 2\n' });
  assert.deepEqual(p.run(), { out: '2\n', err: "minimake: Makefile:3: warning: overriding recipe for target 'x'\n", code: 0 });
});

test('rule lines are expanded when read', (t) => {
  const p = project(t, { Makefile: 'all: $(LATER)\n\t@echo "[$^]"\nLATER = x\n' });
  assert.equal(p.run().out, '[]\n');
});
