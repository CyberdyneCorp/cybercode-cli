'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { project } = require('./helpers');

const info = (t, makefile, files = {}) => {
  const p = project(t, { ...files, Makefile: `${makefile}\nall:\n` });
  const result = p.run();
  assert.equal(result.err, '');
  return result.out.replace("minimake: Nothing to be done for 'all'.\n", '');
};

test('flavors and appending', (t) => {
  assert.equal(info(t, 'A = $(B)\nB = 1\nC := $(B)\nB = 2\nD := x\nD += $(B)\nE ?= e\nE ?= f\n$(info $(A) $(C) $(D) $(E) $(flavor D))'),
    '2 1 x 2 e simple\n');
});

test('recursive self reference', (t) => {
  const p = project(t, { Makefile: 'X = $(X)\n$(info $(X))\n' });
  assert.equal(p.run().err, "minimake: *** Recursive variable 'X' references itself (eventually).  Stop.\n");
});

test('references and substitution references', (t) => {
  assert.equal(info(t, 'A = B\nB = b.c\n$(info $$ $($(A)) ${B} $(B:.c=.o) $(B:%.c=x/%.o))'), '$ b.c b.c b.o x/b.o\n');
});

test('text functions', (t) => {
  assert.equal(info(t, '$(info $(subst a, b,xa)|$(patsubst %.c,%.o,a.c b.h)|$(strip  a  b )|$(filter %.c,a.c b.h)|$(filter-out %.c,a.c b.h)|$(sort b a b))'),
    'x b|a.o b.h|a b|a.c|b.h|a b\n');
  assert.equal(info(t, '$(info $(word 2,a b c)|$(words a b)|$(firstword a b)|$(lastword a b)|$(findstring b,abc))'), 'b|2|a|b|b\n');
});

test('file name functions', (t) => {
  assert.equal(info(t, '$(info $(dir a/b c)|$(notdir a/b c)|$(suffix a.c b)|$(basename a.c b)|$(addprefix p,a b)|$(addsuffix s,a b)|$(join a b,1))'),
    'a/ ./|b c|.c|a b|pa pb|as bs|a1 b\n');
});

test('wildcard is sorted and skips hidden files', (t) => {
  assert.equal(info(t, '$(info $(wildcard *.c))', { 'b.c': '', 'a.c': '', '.h.c': '' }), 'a.c b.c\n');
});

test('foreach, if and call', (t) => {
  assert.equal(info(t, 'f = <$(1)$(2)>\n$(info $(foreach x,a b,$(x)!) $(if ,y,n) $(call f,1))'), 'a! b! n <1>\n');
});

test('shell', (t) => {
  assert.equal(info(t, "$(info [$(shell printf 'a\\nb\\n')])"), '[a b]\n');
});

test('word errors', (t) => {
  const p = project(t, { Makefile: '$(info $(word 0,a))\n' });
  assert.equal(p.run().err, "minimake: *** first argument to 'word' function must be greater than 0.  Stop.\n");
});
