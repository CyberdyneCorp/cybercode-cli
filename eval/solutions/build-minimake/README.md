# minimake

A small make-like build tool for Node 22 with no dependencies. The full contract is in
`SPEC.md`; this is a subset of GNU make (variables of four flavors, conditionals, pattern rules,
order-only prerequisites, a fixed set of functions, `-n`/`-k`/`-B`/`-C`/`-f`).

## Usage

```sh
node minimake.js                       # build the default goal from ./Makefile
node minimake.js app clean             # build the given goals, in order
node minimake.js -f build.mk test      # use another makefile
node minimake.js -C examples/hello     # change directory first
node minimake.js -n                    # dry run: print the commands only
node minimake.js -k all                # keep going after errors
node minimake.js -B                    # rebuild everything
node minimake.js CC=clang CFLAGS=-O2   # command-line variables override the makefile
```

Exit status is 0 on success and 2 on any error.

## Layout

- `minimake.js` — command line.
- `lib/parser.js` — reads the makefile (continuations, comments, conditionals, assignments, rules).
- `lib/expander.js`, `lib/functions.js`, `lib/variables.js` — expansion, built-in functions, variable flavors.
- `lib/builder.js` — rule selection, rebuild decisions and recipe execution.

## Tests

```sh
node --test
```
