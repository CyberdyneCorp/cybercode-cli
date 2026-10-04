# minimake specification

minimake is a small subset of GNU make, written in JavaScript for **Node 22 with no npm
packages** (only `node:` built-in modules). Where this document and GNU make disagree, this
document wins; anything this document does not mention is not supported and not tested.

- Entry point: `minimake.js` at the repository root, run as
  `node minimake.js [options] [NAME=VALUE ...] [targets ...]`. Other modules may live anywhere
  in the repository (for example `lib/`).
- Tests live in `test/` and run with `node --test` from the repository root.
- stdout and stderr are compared **exactly**. minimake must print nothing that this document
  does not ask for — in particular no Node warnings (use CommonJS, or add a `package.json`
  with `"type": "module"`).
- minimake's own output and the output of the commands it runs must appear in the order in which
  they happen: write minimake's messages synchronously (for example with `fs.writeSync`) before
  starting the next command, and let commands write directly to the inherited stdout/stderr.

Throughout, *whitespace* means spaces and tabs, and the *words* of a text are its maximal runs of
non-whitespace characters. Joining words always uses a single space.

## 1. Command line

Arguments are processed left to right; options, assignments and targets may be mixed.

| Argument | Meaning |
|---|---|
| `-f FILE` | Read FILE instead of `Makefile`. |
| `-C DIR` | Change to DIR before doing anything else (before the makefile is read). Each `-C` is applied in turn, relative to the previous one. |
| `-n` | Dry run: print recipe lines, do not run them (§7). |
| `-k` | Keep going after errors (§8). |
| `-B` | Treat every target as out of date (§6). |
| `NAME=VALUE` | Command-line variable (§3.4). Any argument that does not start with `-` and contains `=`; NAME is the text before the first `=` and VALUE everything after it. |
| anything else | A target (goal). |

The flags `-n`, `-k`, `-B` may be combined in one argument (`-nB`). `-f` and `-C` take their value
from the rest of the argument if it is not empty (`-fbuild.mk`, `-nfbuild.mk`), otherwise from
the next argument (`-f build.mk`, `-nf build.mk`).

Usage errors print one line to stderr and exit 2:

- an unknown option letter: `minimake: unknown option '-x'` (the offending letter only, so
  `-nq` reports `'-q'`); an argument starting with `--`: `minimake: unknown option '--foo'`
  (the whole argument);
- `-f` or `-C` with no value: `minimake: option '-f' requires an argument` (or `'-C'`);
- a `-C` directory that cannot be entered: `minimake: *** cannot change to directory 'DIR'.  Stop.`

The makefile is FILE from `-f` (relative to the directory after `-C`), else `Makefile` in the
current directory. If `-f FILE` cannot be read: `minimake: *** cannot read makefile 'FILE'.  Stop.`
If no `-f` is given and `Makefile` does not exist: `minimake: *** No makefile found.  Stop.`
Both exit 2. In messages, *FILE* below is the makefile name exactly as given to `-f`, or `Makefile`.

Environment variables are **not** imported as minimake variables (`$(HOME)` is empty unless the
makefile or the command line defines it). Recipes and `$(shell ...)` run with minimake's own
environment, unchanged.

## 2. Reading the makefile

### 2.1 Lines, continuations and comments

The file is split into lines on `\n`. A line that starts with a TAB character is a *recipe line*
when it is in a recipe context (§2.4); every other line is a *makefile line*.

- **Makefile lines.** If a line ends with a backslash, the backslash, the newline, any whitespace
  before the backslash and any whitespace at the start of the next line are replaced by a single
  space, and the result is processed as one logical line (repeatedly, for several continued
  lines). Then everything from the first `#` to the end of the logical line is a comment and is
  removed (there is no escape for `#`). A logical line that is then empty or only whitespace is
  ignored.
- **Recipe lines.** The leading TAB is removed. If the line ends with a backslash, the next
  physical line is appended to it, with the backslash and the newline kept verbatim and one
  leading TAB of the next line (if present) removed; repeat while the result ends with a
  backslash. `#` is not special in recipe lines: the text is passed to the shell as is.

Line numbers in messages are the 1-based number of the physical line where the logical line
starts.

### 2.2 Classifying a makefile line

1. If the first word is `ifeq`, `ifneq`, `ifdef`, `ifndef`, `else` or `endif`, the line is a
   conditional directive (§2.5).
2. Otherwise find the first `=` or `:` that is outside every variable reference (`$(...)`,
   `${...}`, see §4.1):
   - `:` immediately followed by `=` — a simple assignment `NAME := VALUE`;
   - any other `:` — a rule (§2.3);
   - `=` — an assignment whose operator is `?=` if the `=` is preceded by `?`, `+=` if preceded
     by `+`, and `=` otherwise.
3. Otherwise the line is expanded immediately. If the result is empty or only whitespace, the
   line is done (this is how a line such as `$(info message)` works); otherwise:
   `minimake: FILE:N: *** missing separator.  Stop.` (exit 2).

For an assignment, the variable name is the text before the operator, expanded (§4) and stripped
of surrounding whitespace. The value is the text after the operator with leading and trailing
whitespace removed. §3 says what each operator does.

### 2.3 Rules

A rule line is `TARGETS : PREREQUISITES`. Both sides are expanded immediately, when the line is
read, using the variable values defined so far (variables assigned later in the file are not
seen). TARGETS is split into words. In the expanded PREREQUISITES, a word that is exactly `|`
separates the normal prerequisites (before it) from the order-only prerequisites (after it).

- A rule with several targets is the same as one rule per target with the same prerequisites and
  recipe.
- A target may appear in several rules. Its prerequisites are the concatenation of the
  prerequisites of all its rules, in makefile order. At most one rule should have a recipe: when
  a target that already has a recipe gets another one, minimake prints
  `minimake: FILE:N: warning: overriding recipe for target 'T'` to stderr (N is the line of the
  later rule) and the later recipe replaces the earlier one.
- A name that is both a normal and an order-only prerequisite of a target is a normal
  prerequisite only.
- The prerequisites of the special target `.PHONY` are declared phony; `.PHONY` itself is not a
  real target and may appear in several rules.
- A rule whose first target contains `%` is a pattern rule (§5). Pattern rules have exactly one
  target.
- Target names are plain strings: `a`, `./a` and `dir/../a` are three different targets.

The **default goal** is the first target, in makefile order, of any rule, that does not start
with `.` and does not contain `%`. If no goal is given on the command line and there is no
default goal: `minimake: *** No targets.  Stop.` (exit 2).

### 2.4 Recipes

A recipe context starts at a rule line. Every following line that starts with a TAB belongs to
that rule's recipe. Blank lines, comment-only lines (first non-whitespace character `#`) and
conditional directives do not end the context; any other makefile line (an assignment or another
rule) ends it. A recipe is a list of recipe lines; a rule with no recipe lines has no recipe.

A line starting with a TAB outside a recipe context (before the first rule, or after an
assignment) is an error: `minimake: FILE:N: *** recipe commences before first target.  Stop.`

### 2.5 Conditionals

```
ifeq (A,B)        ifneq (A,B)        ifdef NAME        ifndef NAME
...               ...                ...               ...
else              else               else              else
...               ...                ...               ...
endif             endif              endif             endif
```

- `ifeq (A,B)`: after the keyword, optional whitespace, `(`, then A and B separated by the first
  comma that is outside nested parentheses, then the matching `)` and nothing but whitespace
  after it. A and B are expanded and stripped of leading and trailing whitespace; the condition
  is true when they are equal. `ifneq` is the negation. Any other shape:
  `minimake: FILE:N: *** invalid syntax in conditional.  Stop.`
- `ifdef NAME`: NAME is expanded and stripped; true when that variable is defined with a
  non-empty value (the value itself is not expanded: after `X = $(EMPTY)`, `ifdef X` is true).
  `ifndef` is the negation.
- `else` (optional, at most one per conditional) and `endif` take no arguments.
- Conditionals nest. Lines in a branch that is not taken are skipped entirely (no assignments,
  rules, recipe lines or expansions), except that nested conditional directives are tracked so
  that `else`/`endif` match correctly.
- Errors: `minimake: FILE:N: *** extraneous 'else'.  Stop.` / `... extraneous 'endif'.  Stop.`
  for an `else` or `endif` without an open conditional (or a second `else`), and
  `minimake: FILE:N: *** missing 'endif'.  Stop.` at end of file, where N is the line of the
  innermost unterminated `ifeq`/`ifneq`/`ifdef`/`ifndef`.

All parse errors exit 2 before any recipe runs.

## 3. Variables

### 3.1 Flavors

| Line | Effect |
|---|---|
| `X = v` | X becomes *recursive* with the unexpanded text `v`; it is expanded each time X is used. |
| `X := v` | `v` is expanded now; X becomes *simple* with the result. |
| `X ?= v` | If X is undefined, same as `X = v`; otherwise nothing. A variable assigned an empty value is defined. |
| `X += v` | If X is undefined, same as `X = v`. If X is recursive, the unexpanded `v` is appended; if X is simple, `v` is expanded now and the result appended. Appending adds a single space between the old and the new value, unless the old value is empty. X keeps its flavor. |

Using an undefined variable expands to the empty string.

### 3.2 Recursion

If expanding a recursive variable X requires expanding X again (directly or through other
variables), minimake stops with
`minimake: *** Recursive variable 'X' references itself (eventually).  Stop.` (exit 2), where X
is the variable found to be already in expansion. So `X = $(X) more` fails as soon as X is used;
use `:=` or `+=` instead.

### 3.3 When things are expanded

- Immediately, when the line is read: the right side of `:=`, the appended text of `+=` on a
  simple variable, variable names in assignments, the targets and prerequisites of rules, and
  conditional arguments.
- Deferred: the value of a recursive variable is expanded when it is used; recipe lines are
  expanded when the target's recipe is about to run (§7), so they see the final values of all
  variables.

### 3.4 Command-line variables

`NAME=VALUE` on the command line defines a recursive variable NAME with the unexpanded text
VALUE (no whitespace is removed). Every assignment to NAME in the makefile (`=`, `:=`, `?=`, `+=`)
is ignored. A later `NAME=...` argument replaces an earlier one.

## 4. Expansion

### 4.1 References

| Text | Expands to |
|---|---|
| `$$` | a literal `$` |
| `$(NAME)` or `${NAME}` | the value of NAME |
| `$X` for any other single character X | the value of the variable named X (`$@`, `$a`) |
| `$(NAME:A=B)` | substitution reference, see below |
| `$(FUNCTION ARGS)` | a function call (§4.3) |

A reference started by `$(` ends at the matching `)`, and one started by `${` at the matching
`}`; while looking for it, only nested open/close characters of the same kind are counted. If
there is no matching close character: `minimake: *** unterminated variable reference.  Stop.`
(exit 2).

Inside a reference that is not a function call, the text is expanded first, so names can be
computed: with `A = B` and `B = hello`, `$($(A))` is `hello`. If the expanded text contains `:`
followed later by `=`, it is a substitution reference `NAME:A=B`: the value of NAME, word by
word, with each word ending in A having that ending replaced by B (words not ending in A are
kept); if A contains `%`, it is exactly `$(patsubst A,B,$(NAME))` instead. For example with
`SRC = a.c b.c x.h`, `$(SRC:.c=.o)` is `a.o b.o x.h` and `$(SRC:%.c=obj/%.o)` is
`obj/a.o obj/b.o x.h`.

### 4.2 Lookup order

A name is looked up in this order: the variables bound by `foreach` and `call` while they are
being expanded (innermost first), the automatic variables while a recipe is being expanded
(§7.2), command-line variables, makefile variables.

### 4.3 Functions

`$(` or `${` followed by one of the function names below and then a space or a tab is a function
call; anything else is a variable reference (so `$(foo bar)` is the empty value of a variable
named `foo bar`). The whitespace after the function name is skipped; the rest is split into
arguments at commas that are not inside nested parentheses or braces. Each function has a fixed
number of arguments (listed below); once that many have been found, the remaining text,
commas included, belongs to the last argument. Missing arguments are empty. Apart from the
whitespace skipped after the name, all whitespace in arguments is significant unless the
function's description says it is stripped or the function works on words: `$(subst a, b,xa)`
is `x b`.

Arguments are expanded before the function runs, except where noted for `if`, `foreach` and
`call`. *Pattern* below means a word in which the first `%` matches any string, possibly empty
(a pattern without `%` matches only itself); later `%` characters are literal.

| Function | Result |
|---|---|
| `$(subst FROM,TO,TEXT)` | TEXT with every non-overlapping occurrence of FROM, left to right, replaced by TO. |
| `$(patsubst PATTERN,REPLACEMENT,TEXT)` | PATTERN and REPLACEMENT are stripped of surrounding whitespace. Each word of TEXT that matches PATTERN is replaced by REPLACEMENT with its first `%` replaced by the matched text; other words are unchanged. |
| `$(strip TEXT)` | the words of TEXT. |
| `$(findstring FIND,IN)` | FIND if it occurs in IN, else empty. |
| `$(filter PATTERNS,TEXT)` | the words of TEXT that match at least one word of PATTERNS, in TEXT order. |
| `$(filter-out PATTERNS,TEXT)` | the words of TEXT that match none of the words of PATTERNS. |
| `$(sort LIST)` | the words of LIST sorted by JavaScript string comparison (`<` on UTF-16 code units), duplicates removed. |
| `$(word N,TEXT)` | the N-th word (1-based), or empty if there are fewer. N is stripped; if it is not all digits: `minimake: *** non-numeric first argument to 'word' function: 'N'.  Stop.`; if it is 0: `minimake: *** first argument to 'word' function must be greater than 0.  Stop.` |
| `$(words TEXT)` | the number of words, in decimal. |
| `$(firstword TEXT)` / `$(lastword TEXT)` | the first / last word, or empty. |
| `$(dir NAMES)` | for each word, everything up to and including the last `/`, or `./` if it has none. |
| `$(notdir NAMES)` | for each word, everything after the last `/` (the whole word if it has none). |
| `$(suffix NAMES)` | for each word whose part after the last `/` contains a `.`, the text from the last `.` on; words without one contribute nothing. |
| `$(basename NAMES)` | for each word, the word without its suffix (as defined for `suffix`). |
| `$(addprefix PREFIX,NAMES)` / `$(addsuffix SUFFIX,NAMES)` | each word with PREFIX prepended / SUFFIX appended (PREFIX and SUFFIX are used exactly as given). |
| `$(join LIST1,LIST2)` | the N-th word of LIST1 concatenated with the N-th word of LIST2; extra words of the longer list are kept as they are. |
| `$(wildcard PATTERNS)` | see below. |
| `$(foreach VAR,LIST,TEXT)` | VAR (expanded and stripped) and LIST are expanded; then for each word of LIST, TEXT (unexpanded) is expanded with VAR bound to that word as a simple variable. The results are joined with single spaces. VAR's binding ends when `foreach` returns. |
| `$(if CONDITION,THEN,ELSE)` | CONDITION is expanded and stripped; if non-empty, THEN is expanded and returned, otherwise ELSE (or empty). The branch not taken is not expanded. |
| `$(call NAME,ARG1,ARG2,...)` | any number of arguments. NAME is expanded and stripped, the arguments are expanded; then the value of variable NAME is expanded with `0` bound to NAME, `1` to ARG1, `2` to ARG2 and so on; every other all-digit name (`$(3)` when there are two arguments) is empty during that expansion. The bindings are simple variables. |
| `$(shell COMMAND)` | runs `/bin/sh -c COMMAND` and returns its stdout with trailing newlines removed and every other newline replaced by a space. Its stderr goes to minimake's stderr; its exit status is ignored. |
| `$(info TEXT)` | prints TEXT and a newline to stdout; returns empty. |
| `$(error TEXT)` | stops with `minimake: *** TEXT.  Stop.` (exit 2). |
| `$(flavor NAME)` | NAME is stripped; `undefined`, `recursive` or `simple`. Command-line variables are `recursive`; `foreach` and `call` bindings are `simple`. |

Argument counts: `subst`, `patsubst`, `foreach`, `if`: 3; `findstring`, `filter`, `filter-out`,
`word`, `addprefix`, `addsuffix`, `join`: 2; `call`: unlimited; all others: 1.

**`wildcard`.** Each word of PATTERNS is a glob relative to the current directory (or absolute).
In every `/`-separated component, `*` matches any run of characters, `?` one character, and
`[...]` one character from the set (ranges `a-z` allowed; `[!...]` negates). Wildcard characters
never match `/`, and a component starting with `.` is only matched by a pattern component that
starts with `.`. A word without wildcard characters yields itself if that file or directory
exists. The matches of each word are sorted (JavaScript string comparison); the results of all
words are joined in word order; no matches yield nothing. Results are spelled like the pattern
(`src/*.c` gives `src/a.c`).

## 5. Pattern rules

A pattern rule has one target containing one `%`, for example `%.o: %.c` or
`build/%.o: src/%.c | build`. Pattern rules without a recipe are ignored.

**Matching.** A target name T matches a target pattern `P%S` when T starts with P and ends with
S, and the part between them — the *stem* — is not empty (P and S must not overlap). If the
target pattern contains no `/` and T does, only the part of T after its last `/` is matched;
the directory part D (up to and including the last `/`) is then put back: the stem `$*` is D
followed by the matched part. Example: `lib%.a: %.o` matches `out/libz.a` with stem `out/z`.

**Prerequisites.** In each prerequisite (normal or order-only) of the pattern rule, the first `%`
is replaced by the matched part of the stem; when D was removed during matching, D is prepended
to every prerequisite that contained a `%`. Prerequisites without `%` are used as written.
Example: for `out/libz.a`, `lib%.a: %.o common.h` gives `out/z.o common.h`.

**Choosing a rule.** A target uses a pattern rule only if it is not phony and none of its
explicit rules has a recipe. Among the pattern rules whose target pattern matches and whose
prerequisites all *qualify* — each one exists as a file or directory, or is a target of an
explicit rule — the one with the shortest stem (`$*`) wins; on equal stem lengths, the one that
appears first in the makefile. Pattern rules are not chained: a prerequisite that does not exist
and is not an explicit target does not qualify, even if another pattern rule could make it.

When a pattern rule is chosen for a target that also has explicit rules (without a recipe), the
target's normal prerequisites are the pattern rule's followed by the explicit ones, and likewise
for order-only prerequisites. So with `%.o: %.c` and `main.o: defs.h`, `$^` for `main.o` is
`main.c defs.h`.

## 6. Deciding what to rebuild

minimake updates each goal in command-line order (or the default goal). Updating a target T:

1. If T was already handled during this run, its earlier result is reused (a target is visited
   at most once per run).
2. T's rule is found (§5): a recipe from an explicit rule, or a pattern rule, or no recipe. If T
   has no rule at all (no explicit rule, no pattern rule, not phony): if a file or directory
   named T exists, T is up to date and was not updated; otherwise it is an error (§8):
   `No rule to make target 'T', needed by 'D'` where D is the target whose prerequisite T is,
   or `No rule to make target 'T'` when T is a goal. A phony target with no rule has no
   recipe and no prerequisites.
3. Each normal prerequisite, then each order-only prerequisite, is updated, in order. If a
   prerequisite P is already being updated further up the chain (a cycle), minimake prints
   `minimake: Circular T <- P dependency dropped.` to stderr and removes P from T's
   prerequisites for every purpose (§7.2 included). This includes `a: a`.
4. T is **out of date** when any of these hold:
   - T is phony (phony targets are never looked up in the file system);
   - `-B` was given;
   - no file or directory named T exists;
   - a normal prerequisite was *updated* during this run (see below);
   - a normal prerequisite exists and its modification time is strictly greater than T's
     (equal times do not count).

   Order-only prerequisites never make T out of date.
5. If T is out of date, its recipe runs (§7), if it has one. Either way T then counts as
   *updated* (a target without a recipe that is out of date is updated by doing nothing).
   Otherwise T is not updated. A target that has no rule and exists as a file is never updated.

Modification times are read when they are needed, from the file system, as `mtimeMs`.

After each goal G is handled successfully, if no recipe line was run (or printed, with `-n`)
while handling G, minimake prints to stdout `minimake: 'G' is up to date.` when G has a recipe,
or `minimake: Nothing to be done for 'G'.` when it has none. Recipe lines run for an earlier goal
do not count for a later one.

## 7. Recipes

### 7.1 Running a recipe

When T's recipe runs, **all** of its lines are expanded first, in order (so `$(info ...)` in any
line prints before the first line runs); then the lines run one at a time:

1. Leading whitespace and any of the prefix characters `@` (do not echo), `-` (ignore errors)
   and `+` (run even with `-n`) are removed from the start of the expanded line, in any order and
   combination; prefixes produced by expansion count (`$(Q)echo hi` with `Q = @` is silent). A
   line that is then empty is skipped.
2. Unless `@` was present, the remaining command is printed to stdout followed by a newline
   (exactly as it will be run, including any backslash-newlines).
3. The command runs as `/bin/sh -c COMMAND` in the current directory (after `-C`), with stdin,
   stdout and stderr inherited.
4. If it exits with a non-zero status N: with `-`, minimake prints
   `minimake: [T] Error N (ignored)` to stderr and continues; otherwise T **fails** (§8) and the
   rest of its recipe does not run.

With `-n`, every line is printed (including `@` lines) and not run, except lines with `+`, which
are printed unless `@` and run normally. Targets still count as updated (§6), so their
dependents are printed too; files are not changed.

minimake never deletes or touches target files itself.

### 7.2 Automatic variables

While T's recipe is expanded:

| Variable | Value |
|---|---|
| `$@` | T |
| `$<` | the first normal prerequisite, or empty |
| `$^` | the normal prerequisites with duplicates removed (first occurrence kept) |
| `$+` | the normal prerequisites, duplicates kept |
| `$?` | the normal prerequisites (duplicates removed) that were updated during this run or whose modification time is strictly greater than T's; all of them (duplicates removed) when T is phony or does not exist |
| `$*` | the stem when a pattern rule is used; empty for explicit rules |
| `$(@D)`, `$(@F)` | the directory part of `$@` without the trailing slash (`.` if there is none), and the file part |
| `$(<D)`, `$(<F)` | the same for `$<` |

Order-only prerequisites never appear in automatic variables. Outside recipes automatic variables
are empty.

## 8. Errors and exit status

Without `-k`, the first error stops minimake: a failed recipe line prints
`minimake: *** [T] Error N` to stderr, a missing rule prints
`minimake: *** No rule to make target 'X', needed by 'D'.  Stop.` (or
`minimake: *** No rule to make target 'X'.  Stop.` for a goal); minimake exits 2 at once.

With `-k`, minimake keeps going:

- a failed recipe line prints `minimake: *** [T] Error N` and T fails; a missing rule prints the
  message without `  Stop.` (`minimake: *** No rule to make target 'X', needed by 'D'.`) and
  the target that needed it fails (a goal with no rule fails itself);
- when a prerequisite of T fails, T's remaining prerequisites are still updated, but T's recipe
  does not run and T fails (no message for T itself);
- after a goal G is handled and has failed, minimake prints
  `minimake: Target 'G' not remade because of errors.` to stderr, then goes on with the next goal;
- at the end minimake exits 2 if anything failed.

Exit status: 0 when everything succeeded (including ignored errors), 2 for any error.

Summary of the messages (stdout: the first two; stderr: the rest; `FILE:N` prefixes as described
above):

```
minimake: Nothing to be done for 'G'.
minimake: 'G' is up to date.
minimake: *** [T] Error N
minimake: [T] Error N (ignored)
minimake: *** No rule to make target 'X', needed by 'D'.  Stop.
minimake: *** No rule to make target 'X'.  Stop.
minimake: Target 'G' not remade because of errors.
minimake: Circular T <- P dependency dropped.
minimake: FILE:N: warning: overriding recipe for target 'T'
minimake: FILE:N: *** missing separator.  Stop.
minimake: FILE:N: *** recipe commences before first target.  Stop.
minimake: FILE:N: *** invalid syntax in conditional.  Stop.
minimake: FILE:N: *** extraneous 'else'.  Stop.
minimake: FILE:N: *** extraneous 'endif'.  Stop.
minimake: FILE:N: *** missing 'endif'.  Stop.
minimake: *** Recursive variable 'X' references itself (eventually).  Stop.
minimake: *** unterminated variable reference.  Stop.
minimake: *** No targets.  Stop.
minimake: *** No makefile found.  Stop.
minimake: *** cannot read makefile 'FILE'.  Stop.
minimake: *** cannot change to directory 'DIR'.  Stop.
minimake: unknown option '-x'
minimake: option '-f' requires an argument
```

## 9. Example

```make
CC = cc
SRC := $(wildcard src/*.c)
OBJ = $(patsubst src/%.c,build/%.o,$(SRC))

.PHONY: all clean
all: app

app: $(OBJ)
	$(CC) -o $@ $^

build/%.o: src/%.c | build
	@echo compiling $<
	$(CC) -c -o $@ $<

build:
	mkdir -p $@

clean:
	-rm -rf build app
```

## Done means

- `node minimake.js` behaves exactly as specified above.
- `test/` contains tests that run with `node --test` (at least 20 tests, all passing) covering
  parsing, variables and functions, pattern rules, rebuild decisions, `-n`/`-k`/`-B`/`-C`/`-f`
  and the error messages.
- `README.md` has a `## Usage` section with `node minimake.js` examples that show each of the
  options `-f`, `-C`, `-n`, `-k`, `-B` and a `NAME=VALUE` override.
