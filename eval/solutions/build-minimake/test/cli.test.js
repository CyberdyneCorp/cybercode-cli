'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { project } = require('./helpers');

test('runs the default goal and echoes commands', (t) => {
  const p = project(t, { Makefile: '.PHONY: all\nall:\n\techo hello\n\t@echo quiet\n' });
  assert.deepEqual(p.run(), { out: 'echo hello\nhello\nquiet\n', err: '', code: 0 });
});

test('-f reads another makefile, with attached or separate value', (t) => {
  const p = project(t, { 'b.mk': 'x:\n\t@echo b\n' });
  assert.equal(p.run('-f', 'b.mk').out, 'b\n');
  assert.equal(p.run('-fb.mk').out, 'b\n');
  assert.equal(p.run('-nfb.mk').out, 'echo b\n');
});

test('-C changes directory before reading the makefile', (t) => {
  const p = project(t, { 'sub/Makefile': 'x:\n\ttouch made\n' });
  assert.equal(p.run('-C', 'sub').out, 'touch made\n');
  assert.ok(p.exists('sub/made'));
  assert.deepEqual(p.run('-C', 'nope'), {
    out: '', err: "minimake: *** cannot change to directory 'nope'.  Stop.\n", code: 2,
  });
});

test('usage errors', (t) => {
  const p = project(t, { Makefile: 'x:\n' });
  assert.deepEqual(p.run('-nq'), { out: '', err: "minimake: unknown option '-q'\n", code: 2 });
  assert.deepEqual(p.run('--long'), { out: '', err: "minimake: unknown option '--long'\n", code: 2 });
  assert.deepEqual(p.run('-f'), { out: '', err: "minimake: option '-f' requires an argument\n", code: 2 });
});

test('missing makefiles and goals', (t) => {
  const p = project(t);
  assert.equal(p.run().err, 'minimake: *** No makefile found.  Stop.\n');
  assert.equal(p.run('-f', 'x.mk').err, "minimake: *** cannot read makefile 'x.mk'.  Stop.\n");
  p.write('Makefile', 'X = 1\n');
  assert.deepEqual(p.run(), { out: '', err: 'minimake: *** No targets.  Stop.\n', code: 2 });
});

test('command-line variables override every makefile assignment', (t) => {
  const p = project(t, { Makefile: 'A = a\nA += more\nB := b\nall:\n\t@echo $(A) $(B)\n' });
  assert.equal(p.run('A=x', 'B=$(A)y').out, 'x xy\n');
});

test('environment variables are not imported', (t) => {
  const p = project(t, { Makefile: 'all:\n\t@echo "[$(HOME)]"\n' });
  assert.equal(p.run().out, '[]\n');
});
