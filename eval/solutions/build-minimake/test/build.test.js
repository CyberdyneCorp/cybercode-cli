'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { project, T0 } = require('./helpers');

test('rebuilds only out-of-date targets', (t) => {
  const p = project(t, { Makefile: 't: a b\n\t@echo "build [$?]"\n' });
  p.write('a', '', T0);
  p.write('b', '', T0 + 20);
  p.write('t', '', T0 + 10);
  assert.equal(p.run().out, 'build [b]\n');
  p.write('t', '', T0 + 20);
  assert.equal(p.run().out, "minimake: 't' is up to date.\n");
  assert.equal(p.run('-B').out, 'build []\n');
});

test('nothing to be done for targets without a recipe', (t) => {
  const p = project(t, { Makefile: 'all: f\n' });
  p.write('f', '', T0);
  assert.equal(p.run().out, "minimake: Nothing to be done for 'all'.\n");
});

test('updated prerequisites force dependents', (t) => {
  const p = project(t, { Makefile: 't: p\n\t@echo t\np: q\n\t@echo p\n' });
  p.write('q', '', T0 + 10);
  p.write('p', '', T0);
  p.write('t', '', T0 + 20);
  assert.equal(p.run().out, 'p\nt\n');
});

test('order-only prerequisites do not trigger rebuilds', (t) => {
  const p = project(t, { Makefile: 't: | d\n\t@echo t\nd:\n\t@echo d\n' });
  p.write('t', '', T0);
  assert.equal(p.run().out, 'd\n');
});

test('pattern rules with directory stems and shortest stem', (t) => {
  const p = project(t, { Makefile: 'lib%.a: %.o\n\t@echo $@ $< $*\n%.o: %.c\n\t@echo generic\nsrc/%.o: src/%.c\n\t@echo src $*\n' });
  p.write('out/z.o', '', T0);
  p.write('src/a.c', '', T0);
  assert.equal(p.run('out/libz.a', 'src/a.o').out, 'out/libz.a out/z.o out/z\nsrc a\n');
});

test('pattern rules combine with explicit prerequisites', (t) => {
  const p = project(t, { Makefile: '%.o: %.c\n\t@echo $^\nm.o: m.h\n' });
  p.write('m.c', '', T0);
  p.write('m.h', '', T0);
  assert.equal(p.run('m.o').out, 'm.c m.h\n');
});

test('circular dependencies are dropped', (t) => {
  const p = project(t, { Makefile: 'a: b\n\t@echo a\nb: a\n\t@echo b\n' });
  assert.deepEqual(p.run(), { out: 'b\na\n', err: 'minimake: Circular b <- a dependency dropped.\n', code: 0 });
});

test('recipe failure stops with exit 2', (t) => {
  const p = project(t, { Makefile: 'all: x y\nx:\n\texit 3\ny:\n\techo y\n' });
  assert.deepEqual(p.run(), { out: 'exit 3\n', err: 'minimake: *** [x] Error 3\n', code: 2 });
});

test('-k keeps going and reports failed goals', (t) => {
  const p = project(t, { Makefile: 'all: x y\nx:\n\t@false\ny:\n\t@echo y\n' });
  assert.deepEqual(p.run('-k'), {
    out: 'y\n',
    err: "minimake: *** [x] Error 1\nminimake: Target 'all' not remade because of errors.\n",
    code: 2,
  });
});

test('ignored errors and prefixes', (t) => {
  const p = project(t, { Makefile: 'Q = @\nall:\n\t-false\n\t$(Q)echo done\n' });
  assert.deepEqual(p.run(), { out: 'false\ndone\n', err: 'minimake: [all] Error 1 (ignored)\n', code: 0 });
});

test('missing rules', (t) => {
  const p = project(t, { Makefile: 'all: gone\n' });
  assert.equal(p.run().err, "minimake: *** No rule to make target 'gone', needed by 'all'.  Stop.\n");
  assert.equal(p.run('-k').err,
    "minimake: *** No rule to make target 'gone', needed by 'all'.\nminimake: Target 'all' not remade because of errors.\n");
});

test('-n prints without running, except + lines', (t) => {
  const p = project(t, { Makefile: 'all:\n\t@touch a\n\t+touch b\n' });
  assert.equal(p.run('-n').out, 'touch a\ntouch b\n');
  assert.ok(!fs.existsSync(path.join(p.dir, 'a')));
  assert.ok(fs.existsSync(path.join(p.dir, 'b')));
});

test('automatic variables', (t) => {
  const p = project(t, { Makefile: 'o/t.x: s/a b s/a | c\n\t@echo "$@|$<|$^|$+|$(@D)|$(@F)|$(<D)|$(<F)"\nc:\n' });
  p.write('s/a', '', T0);
  p.write('b', '', T0);
  assert.equal(p.run().out, 'o/t.x|s/a|s/a b|s/a b s/a|o|t.x|s|a\n');
});
