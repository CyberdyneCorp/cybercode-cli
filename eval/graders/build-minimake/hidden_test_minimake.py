"""Hidden behavior tests for build-minimake. Drives `node minimake.js` via subprocess.

The program under test is found through the MINIMAKE_WORKSPACE environment variable. Every test
runs in its own temporary directory; file modification times are set explicitly with os.utime so
rebuild decisions are deterministic. Section numbers refer to SPEC.md.
"""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

WORKSPACE = Path(os.environ["MINIMAKE_WORKSPACE"])
TIMEOUT = 30
T0 = 1_600_000_000  # base modification time; recipes create files with the current time (later)
OLD, MID, NEW = T0, T0 + 100, T0 + 200


class MakeTestCase(unittest.TestCase):
    maxDiff = None

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = Path(tmp.name)

    def write(self, name, text="", mtime=None):
        path = self.dir / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        if mtime is not None:
            os.utime(path, (mtime, mtime))
        return path

    def files(self, **mtimes):
        for name, mtime in mtimes.items():
            self.write(name.replace("__", "/").replace("_dot_", "."), "", mtime)

    def makefile(self, text, name="Makefile"):
        self.write(name, text)

    def mtime(self, name):
        return os.stat(self.dir / name).st_mtime

    def run_make(self, *args, cwd=None, env=None):
        environment = {k: v for k, v in os.environ.items() if k not in ("NODE_OPTIONS", "MAKEFLAGS")}
        environment.update(env or {})
        return subprocess.run(["node", str(WORKSPACE / "minimake.js"), *args], cwd=cwd or self.dir,
                              env=environment, capture_output=True, text=True, timeout=TIMEOUT)

    def expect(self, args, out="", err="", code=0, **kwargs):
        result = self.run_make(*args, **kwargs)
        self.assertEqual(result.stdout, out, f"stdout of minimake {' '.join(args)}")
        self.assertEqual(result.stderr, err, f"stderr of minimake {' '.join(args)}")
        self.assertEqual(result.returncode, code, f"exit code of minimake {' '.join(args)}")
        return result

    def expect_info(self, makefile, lines, args=()):
        """Expand `$(info ...)` lines at parse time; the goal `all` has no recipe."""
        self.makefile(makefile + "\nall:\n")
        self.expect(list(args), "".join(f"{line}\n" for line in lines) + "minimake: Nothing to be done for 'all'.\n")


# --------------------------------------------------------------------------- §1 command line
class CommandLineTest(MakeTestCase):
    def test_default_makefile_and_default_goal(self):
        self.makefile(".PHONY: first second\n%.o: %.c\n\techo pattern\n.hidden:\n\techo hidden\n"
                      "first second:\n\techo $@\n")
        self.expect([], "echo first\nfirst\n")
        self.expect(["second", "first"], "echo second\nsecond\necho first\nfirst\n")

    def test_f_option_forms(self):
        self.makefile("all:\n\t@echo from-Makefile\n")
        self.makefile(".PHONY: all\nall:\n\techo from-build\n", "build.mk")
        self.expect(["-f", "build.mk"], "echo from-build\nfrom-build\n")
        self.expect(["-fbuild.mk"], "echo from-build\nfrom-build\n")
        self.expect(["-nfbuild.mk"], "echo from-build\n")
        self.expect(["-nf", "build.mk"], "echo from-build\n")
        self.expect(["-n", "-f", "build.mk", "all"], "echo from-build\n")

    def test_C_changes_directory_before_reading(self):
        self.write("sub/Makefile", "out: in\n\tcp in out\n\t@echo $(wildcard *)\n")
        self.write("sub/in", "data\n", OLD)
        self.expect(["-C", "sub"], "cp in out\nMakefile in\n")
        self.assertEqual((self.dir / "sub/out").read_text(), "data\n")
        self.expect(["-Csub"], "minimake: 'out' is up to date.\n")

    def test_f_is_relative_to_C_regardless_of_order(self):
        self.write("sub/rules.mk", "x:\n\t@echo in-sub\n")
        self.expect(["-f", "rules.mk", "-C", "sub"], "in-sub\n")

    def test_C_is_applied_in_turn(self):
        self.write("a/b/Makefile", "x:\n\t@echo nested\n")
        self.expect(["-C", "a", "-C", "b"], "nested\n")

    def test_C_missing_directory(self):
        self.expect(["-C", "nope"], err="minimake: *** cannot change to directory 'nope'.  Stop.\n", code=2)

    def test_unknown_options(self):
        self.makefile("x:\n\t@echo x\n")
        self.expect(["-x"], err="minimake: unknown option '-x'\n", code=2)
        self.expect(["-nq"], err="minimake: unknown option '-q'\n", code=2)
        self.expect(["--foo"], err="minimake: unknown option '--foo'\n", code=2)
        self.expect(["--dry-run", "x"], err="minimake: unknown option '--dry-run'\n", code=2)

    def test_option_requires_argument(self):
        self.makefile("x:\n\t@echo x\n")
        self.expect(["-f"], err="minimake: option '-f' requires an argument\n", code=2)
        self.expect(["x", "-C"], err="minimake: option '-C' requires an argument\n", code=2)
        self.expect(["-nf"], err="minimake: option '-f' requires an argument\n", code=2)

    def test_combined_flags(self):
        self.makefile("t: s\n\ttouch t\n")
        self.files(t=NEW, s=OLD)
        self.expect(["-nB"], "touch t\n")
        self.expect(["-Bn"], "touch t\n")
        self.assertEqual(self.mtime("t"), NEW)

    def test_missing_makefiles(self):
        self.expect([], err="minimake: *** No makefile found.  Stop.\n", code=2)
        self.expect(["all"], err="minimake: *** No makefile found.  Stop.\n", code=2)
        self.expect(["-f", "other.mk"], err="minimake: *** cannot read makefile 'other.mk'.  Stop.\n", code=2)

    def test_no_targets(self):
        self.makefile("X = 1\n.PHONY: a\n%.o: %.c\n\tcc $<\n.hidden:\n\t@echo hidden\n")
        self.expect([], err="minimake: *** No targets.  Stop.\n", code=2)
        self.expect([".hidden"], "hidden\n")

    def test_options_assignments_and_goals_mixed(self):
        self.makefile(".PHONY: a b\na:\n\techo a$(X)\nb:\n\techo b$(X)\n")
        self.expect(["b", "-n", "X=1", "a"], "echo b1\necho a1\n")


class CommandLineVariableTest(MakeTestCase):
    def test_overrides_every_assignment_operator(self):
        self.makefile("A = file\nB := file\nC ?= file\nD += file\nD += more\n"
                      "all:\n\t@echo $(A) $(B) $(C) $(D)\n")
        self.expect(["A=1", "B=2", "C=3", "D=4"], "1 2 3 4\n")
        self.expect(["D=4"], "file file file 4\n")

    def test_value_is_literal_and_recursive(self):
        self.makefile("Y = early\nall:\n\t@echo '[$(X)]'\nY = late\n")
        self.expect(["X=$(Y) z"], "[late z]\n")
        self.expect(["X=  two  spaces"], "[  two  spaces]\n")
        self.expect(["X=a=b"], "[a=b]\n")

    def test_later_assignment_wins(self):
        self.makefile("all:\n\t@echo $(X)\n")
        self.expect(["X=1", "X=2"], "2\n")

    def test_command_line_variable_in_rule_and_conditional(self):
        self.makefile("ifdef DEBUG\nMODE = debug\nelse\nMODE = release\nendif\n"
                      "all: $(GOAL)\n\t@echo $(MODE) $^\nprep:\n\t@echo prep\n")
        self.expect(["DEBUG=1", "GOAL=prep"], "prep\ndebug prep\n")
        self.expect(["DEBUG="], "release\n")

    def test_environment_is_not_imported(self):
        self.makefile("all:\n\t@echo '[$(MINIMAKE_TEST_VAR)]' $$MINIMAKE_TEST_VAR\n")
        self.expect([], "[] from-env\n", env={"MINIMAKE_TEST_VAR": "from-env"})


# --------------------------------------------------------------------------- §2 parsing
class ParsingTest(MakeTestCase):
    def test_comments_and_blank_lines(self):
        self.makefile("# leading comment\n\nX = 1 # trailing comment\n   # indented comment\n"
                      "all: # rule comment\n\t@echo '[$(X)]'\n")
        self.expect([], "[1]\n")

    def test_hash_in_recipe_goes_to_the_shell(self):
        self.makefile("all:\n\techo a # shell comment\n\t@echo 'b#c'\n")
        self.expect([], "echo a # shell comment\na\nb#c\n")

    def test_makefile_line_continuation(self):
        self.makefile("X = a   \\\n      b\\\n\tc \\\n  d\nall:\n\t@echo '[$(X)]'\n")
        self.expect([], "[a b c d]\n")

    def test_comment_applies_after_continuation(self):
        self.makefile("X = a \\\n  b # comment \\\n  still comment\nall:\n\t@echo '[$(X)]'\n")
        self.expect([], "[a b]\n")

    def test_recipe_line_continuation(self):
        self.makefile("all:\n\techo one \\\n\ttwo \\\n  three\n\t@echo next\n")
        self.expect([], "echo one \\\ntwo \\\n  three\none two three\nnext\n")

    def test_missing_separator(self):
        self.makefile("X = 1\n\nnot a rule\nall:\n\techo hi\n")
        self.expect([], err="minimake: Makefile:3: *** missing separator.  Stop.\n", code=2)
        self.write("other.mk", "all:\n\t@echo hi\n# c\n\\\n  oops\n")
        self.expect(["-f", "other.mk"], err="minimake: other.mk:4: *** missing separator.  Stop.\n", code=2)

    def test_recipe_before_first_target(self):
        self.makefile("\n\techo hi\nall:\n")
        self.expect([], err="minimake: Makefile:2: *** recipe commences before first target.  Stop.\n", code=2)

    def test_assignment_ends_recipe_context(self):
        self.makefile("all:\n\t@echo a\nX = 1\n\t@echo b\n")
        self.expect([], err="minimake: Makefile:4: *** recipe commences before first target.  Stop.\n", code=2)

    def test_blank_comment_and_conditional_lines_keep_recipe_context(self):
        self.makefile("all:\n\t@echo a\n\n# comment\n\t@echo b\nifdef NOPE\n\t@echo c\nelse\n\t@echo d\nendif\n"
                      "\t@echo e\n")
        self.expect([], "a\nb\nd\ne\n")

    def test_parse_errors_happen_before_any_recipe_runs(self):
        self.makefile("$(info parsed)\nall:\n\ttouch ran\nbad line\n")
        self.expect([], "parsed\n", "minimake: Makefile:4: *** missing separator.  Stop.\n", 2)
        self.assertFalse((self.dir / "ran").exists())

    def test_multiple_targets(self):
        self.makefile("a b: c\n\techo $@ from $<\nc:\n\techo c\n")
        self.expect(["b", "a"], "echo c\nc\necho b from c\nb from c\necho a from c\na from c\n")

    def test_prerequisites_merge_across_rules(self):
        self.makefile(".PHONY: t a b c\nt: a\nt: b c\n\techo $^\nt: a c\na b c:\n\t@echo $@\n")
        self.expect([], "a\nb\nc\necho a b c\na b c\n")

    def test_overriding_recipe_warning(self):
        self.makefile(".PHONY: t\nt:\n\techo first\n\nt:\n\techo second\n")
        self.expect([], "echo second\nsecond\n", "minimake: Makefile:5: warning: overriding recipe for target 't'\n")

    def test_order_only_prerequisites(self):
        self.makefile("t: a | b c\n\techo '[$^] [$<] [$?]'\na b c:\n\t@echo $@\n")
        self.expect([], "a\nb\nc\necho \'[a] [a] [a]\'\n[a] [a] [a]\n")

    def test_name_both_normal_and_order_only_is_normal(self):
        self.makefile("t: a | a b\n\t@echo '[$^]'\n")
        self.files(t=MID, a=NEW, b=OLD)
        self.expect([], "[a]\n")

    def test_rule_lines_are_expanded_when_read(self):
        self.makefile("all: $(OBJS)\n\t@echo '[$^] [$(OBJS)]'\nOBJS = x.o\n")
        self.expect([], "[] [x.o]\n")

    def test_colon_inside_reference_is_not_a_separator(self):
        self.makefile("SRC = a.c b.c\nOBJ := $(SRC:.c=.o)\nprog: $(SRC:.c=.o)\n\t@echo $^ / $(OBJ)\n"
                      "%.o:\n\t@echo build $@\n")
        self.expect([], "build a.o\nbuild b.o\na.o b.o / a.o b.o\n")

    def test_targets_are_plain_strings(self):
        self.makefile("all: ./a a\n\t@echo done\n./a:\n\t@echo dot-a\na:\n\t@echo plain-a\n")
        self.expect([], "dot-a\nplain-a\ndone\n")

    def test_default_goal_skips_dot_and_pattern_targets(self):
        self.makefile(".PHONY: x\n.init:\n\techo init\n%.x: %.y\n\techo p\nlib/real: dep\n\t@echo real\ndep:\n")
        self.expect([], "real\n")

    def test_assignment_name_is_expanded_and_value_trimmed(self):
        self.makefile("P = CC\n$(P)_FLAGS   =   -O2   -g   \nall:\n\t@echo '[$(CC_FLAGS)]'\n")
        self.expect([], "[-O2   -g]\n")

    def test_phony_in_several_rules(self):
        self.makefile(".PHONY: a\n.PHONY: b\na b:\n\t@echo $@\n")
        self.files(a=OLD, b=OLD)
        self.expect(["a", "b"], "a\nb\n")


class ConditionalTest(MakeTestCase):
    def test_ifeq_ifneq_with_else(self):
        self.expect_info("X = 1\nifeq ($(X),1)\nA = yes\nelse\nA = no\nendif\n"
                         "ifneq ($(X),1)\nB = yes\nelse\nB = no\nendif\n"
                         "ifeq ($(X),2)\nC = yes\nendif\n$(info $(A) $(B) [$(C)])", ["yes no []"])

    def test_ifeq_arguments_are_stripped(self):
        self.expect_info("S = a \nifeq ( a ,$(S))\n$(info stripped)\nendif\nifeq (,)\n$(info empty)\nendif\n"
                         "ifeq (a b,a  b)\n$(info same)\nelse\n$(info inner-space-kept)\nendif",
                         ["stripped", "empty", "inner-space-kept"])

    def test_ifeq_with_nested_parentheses(self):
        self.expect_info("ifeq ($(subst a,b,aa),bb)\n$(info subst-ok)\nendif\n"
                         "ifeq ((x),(x))\n$(info parens-ok)\nendif", ["subst-ok", "parens-ok"])

    def test_ifdef_and_ifndef(self):
        self.expect_info("FULL = 1\nEMPTY =\nREF = $(EMPTY)\n"
                         "ifdef FULL\n$(info full)\nendif\nifdef EMPTY\n$(info empty)\nendif\n"
                         "ifdef REF\n$(info ref)\nendif\nifdef NEVER\n$(info never)\nendif\n"
                         "ifndef NEVER\n$(info not-never)\nendif\nN = FULL\nifdef $(N)\n$(info computed)\nendif",
                         ["full", "ref", "not-never", "computed"])

    def test_nested_conditionals_and_skipped_branches(self):
        self.expect_info("ifdef NOPE\n$(error should not expand)\nifeq ($(info no),)\nX = 1\nelse\nX = 2\nendif\n"
                         "bogus line without separator\nelse\nifeq (a,a)\nX = 3\nelse\nX = 4\nendif\nendif\n"
                         "$(info X=$(X))", ["X=3"])

    def test_skipped_branch_has_no_rules_or_recipes(self):
        self.makefile("ifdef NOPE\nfirst:\n\t@echo first\n\techo also\nendif\nsecond:\n\t@echo second\n")
        self.expect([], "second\n")
        self.expect(["NOPE=1"], "first\necho also\nalso\n")

    def test_conditional_errors(self):
        cases = [
            ("all:\nelse\n", "Makefile:2: *** extraneous 'else'"),
            ("ifdef X\nelse\nelse\nendif\n", "Makefile:3: *** extraneous 'else'"),
            ("X = 1\nendif\n", "Makefile:2: *** extraneous 'endif'"),
            ("ifdef A\nifeq (1,1)\nendif\n\n", "Makefile:1: *** missing 'endif'"),
            ("ifdef A\nendif\nifdef B\nifeq (1,1)\n", "Makefile:4: *** missing 'endif'"),
            ("ifeq (a b)\nendif\n", "Makefile:1: *** invalid syntax in conditional"),
            ("ifeq a,b\nendif\n", "Makefile:1: *** invalid syntax in conditional"),
            ("ifeq (a,b) x\nendif\n", "Makefile:1: *** invalid syntax in conditional"),
        ]
        for text, message in cases:
            with self.subTest(makefile=text):
                self.makefile(text)
                self.expect([], err=f"minimake: {message}.  Stop.\n", code=2)


# --------------------------------------------------------------------------- §3 variables
class VariableTest(MakeTestCase):
    def test_recursive_and_simple_flavors(self):
        self.expect_info("A = $(B)\nB = 1\nC := $(B)\nB = 2\n$(info $(A) $(C))", ["2 1"])

    def test_conditional_assignment(self):
        self.expect_info("E =\nE ?= set\nU ?= $(V)\nV = late\n$(info [$(E)] [$(U)])", ["[] [late]"])

    def test_append(self):
        self.expect_info("R = a $(V)\nR += b $(V)\nV = 1\nS := s $(V)\nS += t $(W)\nW = 2\n"
                         "N += first\nE =\nE += x\nE2 :=\nE2 += y\n"
                         "$(info [$(R)] [$(S)] [$(N)] [$(E)] [$(E2)])\nV = 9\n$(info [$(R)] [$(S)])",
                         ["[a 1 b 1] [s 1 t ] [first] [x] [y]", "[a 9 b 9] [s 1 t ]"])

    def test_append_keeps_flavor(self):
        self.expect_info("A = 1\nA += 2\nB := 1\nB += 2\nC += 1\n"
                         "$(info $(flavor A) $(flavor B) $(flavor C) $(flavor D) $(flavor CLI))",
                         ["recursive simple recursive undefined recursive"], args=["CLI=x"])

    def test_recursive_self_reference_is_an_error(self):
        self.makefile("X = $(X) more\nall:\n\t@echo $(X)\n")
        self.expect([], err="minimake: *** Recursive variable 'X' references itself (eventually).  Stop.\n", code=2)
        self.makefile("A = $(B)\nB = x$(A)\n$(info $(A))\nall:\n")
        self.expect([], err="minimake: *** Recursive variable 'A' references itself (eventually).  Stop.\n", code=2)

    def test_simple_self_reference_is_fine(self):
        self.expect_info("X = a\nX := $(X) more\nY = b\nY += $(Y:b=c)\n$(info $(X))", ["a more"])

    def test_recipes_see_final_values(self):
        self.makefile("all:\n\t@echo $(MSG)\nMSG = early\nMSG = final\n")
        self.expect([], "final\n")


# --------------------------------------------------------------------------- §4 expansion
class ExpansionTest(MakeTestCase):
    def test_dollar_dollar_and_single_character_names(self):
        self.makefile("X = ex\nall:\n\t@printf '%s\\n' '$$X' $X ${X} $(X)\n")
        self.expect([], "$X\nex\nex\nex\n")

    def test_computed_names(self):
        self.expect_info("A = B\nB = hello\nB_1 = one\nN = 1\n$(info $($(A)) $(B_$(N)) ${$(A)})", ["hello one hello"])

    def test_substitution_references(self):
        self.expect_info("SRC = a.c b.c x.h c.cc\n$(info $(SRC:.c=.o))\n$(info $(SRC:%.c=obj/%.o))\n"
                         "$(info ${SRC:.h=})\nEXT = .c\n$(info $(SRC:$(EXT)=.s))",
                         ["a.o b.o x.h c.cc", "obj/a.o obj/b.o x.h c.cc", "a.c b.c x c.cc", "a.s b.s x.h c.cc"])

    def test_unterminated_reference(self):
        self.makefile("X = $(Y\nall:\n\t@echo $(X)\n")
        self.expect([], err="minimake: *** unterminated variable reference.  Stop.\n", code=2)

    def test_unknown_function_is_a_variable(self):
        self.expect_info("$(info [$(foo bar)] [$(subst)])", ["[] []"])

    def test_braces_and_parentheses_count_separately(self):
        self.expect_info("X = 1\n$(info ${X} $(patsubst %,{%},a b) ${patsubst %,(%),c} ${X:1=(2)})",
                         ["1 {a} {b} (c) (2)"])

    def test_undefined_and_automatic_outside_recipes_are_empty(self):
        self.expect_info("A := [$@][$<][$(UNDEFINED)]\n$(info $(A))", ["[][][]"])


class FunctionTest(MakeTestCase):
    def test_subst(self):
        self.expect_info("$(info [$(subst ee,EE,feet on the street)])\n$(info [$(subst a, b,xa)])\n"
                         "$(info [$(subst aa,b,aaa)])\n$(info [$(subst a,b,x,a)])",
                         ["[fEEt on the strEEt]", "[x b]", "[ba]", "[x,b]"])

    def test_patsubst(self):
        self.expect_info("$(info [$(patsubst %.c,%.o,a.c  b.c c.h)])\n$(info [$(patsubst %.c, %.o ,a.c)])\n"
                         "$(info [$(patsubst a.c,X,a.c b.c)])\n$(info [$(patsubst a%,x%y,a ab)])\n"
                         "$(info [$(patsubst %,%%.o,p)])",
                         ["[a.o b.o c.h]", "[a.o]", "[X b.c]", "[xy xby]", "[p%.o]"])

    def test_strip_and_findstring(self):
        self.expect_info("$(info [$(strip   a   b  c  )])\n$(info [$(findstring a,cat)] [$(findstring x,cat)])",
                         ["[a b c]", "[a] []"])

    def test_filter_and_filter_out(self):
        self.expect_info("L = a.c b.h c.s d.c a.c\n$(info [$(filter %.c %.s,$(L))])\n"
                         "$(info [$(filter-out %.c b.h,$(L))])\n$(info [$(filter a%,a abc ba)])",
                         ["[a.c c.s d.c a.c]", "[c.s]", "[a abc]"])

    def test_sort(self):
        self.expect_info("$(info [$(sort foo bar Baz lose bar a10 a9)])", ["[Baz a10 a9 bar foo lose]"])

    def test_word_functions(self):
        self.expect_info("L = one two  three\n$(info [$(word 2,$(L))] [$(word 4,$(L))] [$(word  3 ,$(L))])\n"
                         "$(info [$(words $(L))] [$(words )] [$(firstword $(L))] [$(lastword $(L))] [$(lastword )])",
                         ["[two] [] [three]", "[3] [0] [one] [three] []"])

    def test_word_errors(self):
        self.makefile("$(info $(word 0,a b))\nall:\n")
        self.expect([], err="minimake: *** first argument to 'word' function must be greater than 0.  Stop.\n", code=2)
        self.makefile("$(info $(word x1,a b))\nall:\n")
        self.expect([], err="minimake: *** non-numeric first argument to 'word' function: 'x1'.  Stop.\n", code=2)

    def test_file_name_functions(self):
        self.expect_info("N = src/a.c b /abs/c.d.e x.y/z lib/\n$(info [$(dir $(N))])\n$(info [$(notdir src/a.c b x.y/z)])\n"
                         "$(info [$(suffix $(N))])\n$(info [$(basename $(N))])",
                         ["[src/ ./ /abs/ x.y/ lib/]", "[a.c b z]", "[.c .e]", "[src/a b /abs/c.d x.y/z lib/]"])

    def test_addprefix_addsuffix_join(self):
        self.expect_info("$(info [$(addprefix  src/,a b)] [$(addprefix x ,a b)] [$(addsuffix .o,a  b)])\n"
                         "$(info [$(join a b c,.x .y)] [$(join a,1 2 3)])",
                         ["[src/a src/b] [x a x b] [a.o b.o]", "[a.x b.y c] [a1 2 3]"])

    def test_foreach(self):
        self.expect_info("D = x y\nV = outer\n$(info [$(foreach V,$(D),$(V).o)])\n$(info [$(V)])\n"
                         "N = V\n$(info [$(foreach $(N), a b ,<$(V)>)])\n"
                         "$(info [$(foreach a,1 2,$(foreach b,x y,$(a)$(b)))])\n$(info [$(foreach V,,never)])",
                         ["[x.o y.o]", "[outer]", "[<a> <b>]", "[1x 1y 2x 2y]", "[]"])

    def test_if(self):
        self.expect_info("$(info [$(if x,yes,no)] [$(if ,yes,no)] [$(if   ,yes)] [$(if $(EMPTY) ,yes,no)])\n"
                         "$(info [$(if 1,ok,$(error not lazy))] [$(if ,$(error not lazy),else)])\n"
                         "$(info [$(if a, b ,c)])",
                         ["[yes] [no] [] [no]", "[ok] [else]", "[ b ]"])

    def test_call(self):
        self.expect_info("pair = $(1)-$(2)[$(3)]\nname = $(0)\nouter = $(call inner,x)$(3)\ninner = <$(1)$(2)>\n"
                         "$(info $(call pair,a,b) $(call pair,a,b,c,d) $(call name) $(call  pair , x ,y))\n"
                         "$(info $(call outer,1,2,3))\n$(info [$(call undefined,1)])\nrev = $2 $1\n$(info $(call rev,a,b))",
                         ["a-b[] a-b[c] name  x -y[]", "<x>3", "[]", "b a"])

    def test_shell(self):
        self.expect_info("$(info [$(shell printf 'a\\nb\\n\\n\\n')])\n$(info [$(shell exit 3)])\n"
                         "$(info [$(shell printf ' x ')])", ["[a b]", "[]", "[ x ]"])

    def test_shell_stderr_passes_through(self):
        self.makefile("X := $(shell echo to-stderr >&2)\nall:\n")
        self.expect([], "minimake: Nothing to be done for 'all'.\n", "to-stderr\n")

    def test_info_and_error(self):
        self.makefile("$(info first)\nX = $(error broken $(Y))\nY = value\n$(info second)\nall:\n\ttouch out\n\t@echo $(X)\n")
        self.expect([], "first\nsecond\n", "minimake: *** broken value.  Stop.\n", 2)
        self.assertFalse((self.dir / "out").exists())

    def test_last_argument_keeps_commas(self):
        self.expect_info("comma := ,\n$(info [$(subst $(comma), ,a,b,c)])\n$(info [$(addsuffix .x,a,b c)])\n"
                         "$(info [$(if 1,a,b,c)])\n$(info [$(subst a,b,f(a,a)a)])",
                         ["[a b c]", "[a,b.x c.x]", "[a]", "[f(b,b)b]"])


class WildcardTest(MakeTestCase):
    def setUp(self):
        super().setUp()
        for name in ["b.c", "a.c", "C.c", "a.h", ".hidden.c", "src/x.c", "src/y.c", "src/.z.c", "lib/x.c",
                     "lib/sub/w.c", "ab1.c", "ab2.c", "abc.c"]:
            self.write(name)

    def test_sorted_and_hidden(self):
        self.expect_info("$(info [$(wildcard *.c)])\n$(info [$(wildcard .*.c)])",
                         ["[C.c a.c ab1.c ab2.c abc.c b.c]", "[.hidden.c]"])

    def test_patterns_keep_their_order_and_spelling(self):
        self.expect_info("$(info [$(wildcard src/*.c *.h ./a.*)])\n$(info [$(wildcard */x.c)])\n"
                         "$(info [$(wildcard */*/*.c)])",
                         ["[src/x.c src/y.c a.h ./a.c ./a.h]", "[lib/x.c src/x.c]", "[lib/sub/w.c]"])

    def test_question_mark_and_sets(self):
        self.expect_info("$(info [$(wildcard ab?.c)])\n$(info [$(wildcard ab[12].c)] [$(wildcard ab[!1].c)])\n"
                         "$(info [$(wildcard [a-b].c)])",
                         ["[ab1.c ab2.c abc.c]", "[ab1.c ab2.c] [ab2.c abc.c]", "[a.c b.c]"])

    def test_literal_names_and_no_matches(self):
        self.expect_info("$(info [$(wildcard a.h missing.h src lib/sub/w.c)])\n$(info [$(wildcard *.zz)])",
                         ["[a.h src lib/sub/w.c]", "[]"])

    def test_absolute_patterns(self):
        base = str(self.dir.resolve())
        self.expect_info(f"$(info [$(wildcard {base}/src/*.c)])", [f"[{base}/src/x.c {base}/src/y.c]"])


# --------------------------------------------------------------------------- §5 pattern rules
class PatternRuleTest(MakeTestCase):
    def test_basic_pattern_rule(self):
        self.makefile("prog: main.o util.o\n\t@echo link $^\n%.o: %.c\n\t@echo cc $< -o $@ stem=$*\n")
        self.files(main_dot_c=OLD, util_dot_c=OLD)
        self.expect([], "cc main.c -o main.o stem=main\ncc util.c -o util.o stem=util\nlink main.o util.o\n")

    def test_directory_part_is_removed_for_patterns_without_slash(self):
        self.makefile("lib%.a: %.o common.h\n\t@echo '[$@] [$^] [$*]'\n")
        self.files(out__z_dot_o=OLD, common_dot_h=OLD)
        self.expect(["out/libz.a"], "[out/libz.a] [out/z.o common.h] [out/z]\n")

    def test_pattern_with_slash_matches_full_name(self):
        self.makefile("build/%.o: src/%.c\n\t@echo $< '->' $@ '($*)'\n")
        self.files(src__sub__a_dot_c=OLD)
        self.expect(["build/sub/a.o"], "src/sub/a.c -> build/sub/a.o (sub/a)\n")

    def test_shortest_stem_wins_then_first(self):
        self.makefile("%.o: %.c\n\t@echo generic $*\nsrc/%.o: src/%.c\n\t@echo src $*\n"
                      "src/%.o: src/%.c\n\t@echo src-second $*\nx%.o: x%.c\n\t@echo x-rule $*\n")
        self.files(src__a_dot_c=OLD, xy_dot_c=OLD)
        self.expect(["src/a.o", "xy.o"], "src a\nx-rule y\n")

    def test_rule_with_non_qualifying_prerequisite_is_skipped(self):
        self.makefile("%.o: %.c gen.h\n\t@echo first\n%.o: %.c\n\t@echo second\n%.o: %.s\n\t@echo third\n")
        self.files(a_dot_c=OLD, b_dot_s=OLD)
        self.expect(["a.o", "b.o"], "second\nthird\n")

    def test_prerequisite_qualifies_as_explicit_target(self):
        self.makefile("%.o: %.c gen.h\n\t@echo cc $^\ngen.h:\n\t@echo generate\n")
        self.files(a_dot_c=OLD)
        self.expect(["a.o"], "generate\ncc a.c gen.h\n")

    def test_no_chaining(self):
        self.makefile("%.o: %.c\n\t@echo cc\n%.c: %.y\n\t@echo yacc\n")
        self.files(a_dot_y=OLD)
        self.expect(["a.o"], err="minimake: *** No rule to make target 'a.o'.  Stop.\n", code=2)
        self.expect(["a.c"], "yacc\n")

    def test_pattern_rules_apply_to_existing_files(self):
        self.makefile("%.o: %.c\n\t@echo cc $<\n%.c: %.y\n\t@echo yacc $< '>' $@\n")
        self.files(a_dot_y=NEW, a_dot_c=MID, a_dot_o=MID)
        self.expect(["a.o"], "yacc a.y > a.c\ncc a.c\n")

    def test_explicit_prerequisites_follow_pattern_ones(self):
        self.makefile("%.o: %.c | dir\n\t@echo '[$^] [$<]'\nmain.o: defs.h\nmain.o: | other\ndir other:\n\t@echo mk $@\n")
        self.files(main_dot_c=OLD, defs_dot_h=OLD)
        self.expect(["main.o"], "mk dir\nmk other\n[main.c defs.h] [main.c]\n")

    def test_explicit_recipe_beats_pattern(self):
        self.makefile("%.o: %.c\n\t@echo pattern\nspecial.o: special.c\n\t@echo explicit $^\n")
        self.files(special_dot_c=OLD)
        self.expect(["special.o"], "explicit special.c\n")

    def test_phony_targets_do_not_use_pattern_rules(self):
        self.makefile(".PHONY: a.o\n%.o: %.c\n\t@echo pattern\n")
        self.files(a_dot_c=OLD)
        self.expect(["a.o"], "minimake: Nothing to be done for 'a.o'.\n")

    def test_pattern_rules_without_recipe_are_ignored(self):
        self.makefile("%.o: %.c\n%.o: %.s\n\t@echo asm $<\n")
        self.files(a_dot_c=OLD, a_dot_s=OLD)
        self.expect(["a.o"], "asm a.s\n")

    def test_stem_must_not_be_empty(self):
        self.makefile("a%: x\n\t@echo matched $*\n")
        self.files(x=OLD)
        self.expect(["ab"], "matched b\n")
        self.expect(["a"], err="minimake: *** No rule to make target 'a'.  Stop.\n", code=2)

    def test_pattern_order_only_prerequisites(self):
        self.makefile("out/%.txt: %.in | out\n\t@echo '[$^] make $@'\nout:\n\t@echo mkdir out\n")
        self.files(a_dot_in=OLD)
        self.expect(["out/a.txt"], "mkdir out\n[a.in] make out/a.txt\n")

    def test_pattern_target_rebuilt_only_when_out_of_date(self):
        self.makefile("%.o: %.c\n\t@echo cc $<\n")
        self.files(a_dot_c=OLD, a_dot_o=MID, b_dot_c=NEW, b_dot_o=MID)
        self.expect(["a.o", "b.o"], "minimake: 'a.o' is up to date.\ncc b.c\n")


# --------------------------------------------------------------------------- §6 rebuild decisions
class RebuildTest(MakeTestCase):
    MK = "t: a b\n\t@echo 'build $@ newer=[$?]'\n"

    def test_missing_target_is_built(self):
        self.makefile(self.MK)
        self.files(a=OLD, b=OLD)
        self.expect([], "build t newer=[a b]\n")

    def test_up_to_date_when_target_is_newer(self):
        self.makefile(self.MK)
        self.files(a=OLD, b=MID, t=NEW)
        self.expect([], "minimake: 't' is up to date.\n")

    def test_equal_times_are_up_to_date(self):
        self.makefile(self.MK)
        self.files(a=MID, b=MID, t=MID)
        self.expect([], "minimake: 't' is up to date.\n")

    def test_newer_prerequisite(self):
        self.makefile(self.MK)
        self.files(a=OLD, b=MID + 1, t=MID)
        self.expect([], "build t newer=[b]\n")

    def test_dollar_question_deduplicates_when_target_missing(self):
        self.makefile("t: a b a\n\t@echo '[$?] [$^] [$+]'\n")
        self.files(a=OLD, b=OLD)
        self.expect([], "[a b] [a b] [a b a]\n")

    def test_updated_prerequisite_rebuilds_dependent_even_without_newer_file(self):
        self.makefile("t: p\n\t@echo 'build t [$?]'\np: q\n\t@echo refresh p\n")
        self.files(q=MID, p=OLD, t=NEW)
        self.expect([], "refresh p\nbuild t [p]\n")
        self.assertEqual(self.mtime("p"), OLD)

    def test_order_only_never_triggers_but_is_built(self):
        self.makefile("t: a | dir\n\t@echo build t\ndir:\n\t@echo make dir\n")
        self.files(a=OLD, t=MID)
        self.write("dir/x", "", NEW)
        os.utime(self.dir / "dir", (NEW, NEW))
        self.expect([], "minimake: 't' is up to date.\n")
        self.makefile("t: a | missing\n\t@echo build t\nmissing:\n\t@echo make missing\n")
        self.expect([], "make missing\n")

    def test_phony_targets(self):
        self.makefile(".PHONY: all clean\nall: t\n\t@echo all\nclean:\n\t@echo clean\nt: all-file\n\t@echo t\n"
                      "u: clean\n\t@echo 'u [$?]'\n")
        self.files(t=NEW, clean=NEW, u=NEW)
        self.write("all-file", "", OLD)
        self.expect(["all", "clean", "u"], "all\nclean\nu [clean]\n")

    def test_always_make(self):
        self.makefile("t: a\n\t@echo 't [$?]'\na:\n\t@echo a\nb: t\n\t@echo b\n")
        self.files(a=OLD, t=MID, b=NEW)
        self.expect(["-B", "b"], "a\nt [a]\nb\n")
        self.expect(["b"], "minimake: 'b' is up to date.\n")

    def test_existing_file_without_rule(self):
        self.makefile("t:\n\t@echo t\n")
        self.files(src=OLD)
        self.expect(["src"], "minimake: Nothing to be done for 'src'.\n")
        self.expect(["-B", "src"], "minimake: Nothing to be done for 'src'.\n")

    def test_target_without_recipe(self):
        self.makefile("all: lib\nlib: x.o\nx.o: x.c\n\t@echo cc\nother: lib\n\t@echo other\n")
        self.files(x_dot_c=OLD, x_dot_o=MID, other=NEW, lib=NEW)
        self.expect(["all"], "minimake: Nothing to be done for 'all'.\n")
        self.expect(["other"], "minimake: 'other' is up to date.\n")
        self.files(x_dot_c=NEW + 1)
        self.expect(["other"], "cc\nother\n")

    def test_target_without_recipe_and_missing_file_is_updated(self):
        self.makefile("out: stamp\n\t@echo out\nstamp:\n")
        self.files(out=NEW)
        self.expect([], "out\n")

    def test_each_target_visited_once(self):
        self.makefile("top: l r\n\t@echo top\nl: base\n\t@echo l\nr: base\n\t@echo r\nbase:\n\t@echo base\n")
        self.expect(["top", "base"], "base\nl\nr\ntop\nminimake: 'base' is up to date.\n")

    def test_messages_are_per_goal(self):
        self.makefile(".PHONY: a\na:\n\t@echo a\nb: a\nc: f\n\techo c\nd:\n")
        self.files(f=OLD, c=NEW)
        self.expect(["a", "b", "c", "d"], "a\nminimake: Nothing to be done for 'b'.\n"
                    "minimake: 'c' is up to date.\nminimake: Nothing to be done for 'd'.\n")

    def test_circular_dependencies_are_dropped(self):
        self.makefile("a: b\n\t@echo 'a [$^]'\nb: c a\n\t@echo 'b [$^]'\nc:\n\t@echo c\n")
        self.expect([], "c\nb [c]\na [b]\n", "minimake: Circular b <- a dependency dropped.\n")

    def test_self_dependency(self):
        self.makefile("a: a x\n\t@echo 'a [$^] [$<]'\nx:\n")
        self.expect([], "a [x] [x]\n", "minimake: Circular a <- a dependency dropped.\n")

    def test_directory_prerequisites_use_their_time(self):
        self.makefile("t: d\n\t@echo t\n")
        (self.dir / "d").mkdir()
        os.utime(self.dir / "d", (NEW, NEW))
        self.files(t=MID)
        self.expect([], "t\n")

    def test_existing_goal_without_rule_dependencies(self):
        self.makefile("t: in\n\tcat in > t\n")
        self.write("in", "v1\n", MID)
        self.files(t=OLD)
        self.expect([], "cat in > t\n")
        self.assertEqual((self.dir / "t").read_text(), "v1\n")
        self.expect([], "minimake: 't' is up to date.\n")


# --------------------------------------------------------------------------- §7 recipes
class RecipeTest(MakeTestCase):
    def test_echo_and_silent(self):
        self.makefile("Q = @\nall:\n\techo loud\n\t@echo quiet\n\t$(Q)echo expanded-quiet\n")
        self.expect([], "echo loud\nloud\nquiet\nexpanded-quiet\n")

    def test_prefix_combinations(self):
        self.makefile("all:\n\t@-false\n\t-@false\n\t @ - echo spaced\n\t-  echo ign\n")
        self.expect([], "spaced\necho ign\nign\n",
                    "minimake: [all] Error 1 (ignored)\nminimake: [all] Error 1 (ignored)\n")

    def test_empty_expanded_lines_are_skipped(self):
        self.makefile("t:\n\t$(EMPTY)\n\t@$(EMPTY)\n")
        self.expect([], "minimake: 't' is up to date.\n")

    def test_all_lines_are_expanded_before_the_first_runs(self):
        self.makefile("all:\n\techo one\n\techo two$(info expanded)\n")
        self.expect([], "expanded\necho one\none\necho two\ntwo\n")

    def test_failure_stops_everything(self):
        self.makefile("all: a b\n\t@echo all\na:\n\t@echo a1\n\texit 3\n\t@echo a2\nb:\n\t@echo b\n")
        self.expect([], "a1\nexit 3\n", "minimake: *** [a] Error 3\n", 2)

    def test_ignored_error_continues(self):
        self.makefile("all:\n\t-exit 4\n\t@echo after\n")
        self.expect([], "exit 4\nafter\n", "minimake: [all] Error 4 (ignored)\n", 0)

    def test_each_line_runs_in_its_own_shell(self):
        self.makefile("all:\n\t@X=1; echo \"[$$X]\"\n\t@echo \"[$$X]\"\n\t@cd /; true\n\t@ls Makefile\n")
        self.expect([], "[1]\n[]\nMakefile\n")

    def test_recipe_inherits_environment_and_stderr(self):
        self.makefile("all:\n\t@echo $$MM_VALUE >&2\n")
        self.expect([], err="xyz\n", env={"MM_VALUE": "xyz"})

    def test_dry_run(self):
        self.makefile("all: t\n\t@echo all\nt: s\n\ttouch t\n\t@echo silent-too\n")
        self.files(s=NEW, t=OLD)
        self.expect(["-n"], "touch t\necho silent-too\necho all\n")
        self.assertEqual(self.mtime("t"), OLD)

    def test_dry_run_runs_plus_lines(self):
        self.makefile("all:\n\t+echo forced > forced.txt\n\t+@echo forced-silent\n\ttouch skipped\n")
        self.expect(["-n"], "echo forced > forced.txt\nforced-silent\ntouch skipped\n")
        self.assertTrue((self.dir / "forced.txt").exists())
        self.assertFalse((self.dir / "skipped").exists())

    def test_dry_run_when_up_to_date(self):
        self.makefile("t: s\n\ttouch t\n")
        self.files(s=OLD, t=NEW)
        self.expect(["-n"], "minimake: 't' is up to date.\n")

    def test_automatic_variables(self):
        self.makefile("out/prog.bin: src/m.c lib.a src/m.c | odir\n"
                      "\t@echo '[$@] [$<] [$^] [$+] [$*] [$(@D)] [$(@F)] [$(<D)] [$(<F)]'\nodir:\n")
        self.files(src__m_dot_c=OLD, lib_dot_a=OLD)
        self.expect([], "[out/prog.bin] [src/m.c] [src/m.c lib.a] [src/m.c lib.a src/m.c] [] [out] [prog.bin] [src] [m.c]\n")

    def test_automatic_directory_parts_without_slash(self):
        self.makefile("t: a\n\t@echo '[$(@D)] [$(@F)] [$(<D)] [$(<F)]'\n")
        self.files(a=OLD)
        self.expect(["t"], "[.] [t] [.] [a]\n")

    def test_output_interleaves_in_order(self):
        lines = "".join(f"\techo line{i}\n" for i in range(20))
        self.makefile("all: a b\n\t@echo all-out; echo all-err >&2\na:\n" + lines + "b:\n\techo b1 && echo b2\n")
        expected = "".join(f"echo line{i}\nline{i}\n" for i in range(20)) + "echo b1 && echo b2\nb1\nb2\nall-out\n"
        self.expect([], expected, "all-err\n")


# --------------------------------------------------------------------------- §8 errors and -k
class ErrorTest(MakeTestCase):
    def test_missing_prerequisite_without_rule(self):
        self.makefile("all: built missing\n\t@echo all\nbuilt:\n\t@echo built\n")
        self.expect([], "built\n", "minimake: *** No rule to make target 'missing', needed by 'all'.  Stop.\n", 2)

    def test_missing_goal(self):
        self.makefile("all:\n\t@echo all\n")
        self.expect(["nope", "all"], err="minimake: *** No rule to make target 'nope'.  Stop.\n", code=2)

    def test_missing_order_only_prerequisite(self):
        self.makefile("t: | gone\n\t@echo t\n")
        self.expect([], err="minimake: *** No rule to make target 'gone', needed by 't'.  Stop.\n", code=2)

    def test_keep_going_after_recipe_failure(self):
        self.makefile(".PHONY: all x y z\nall: x y z\n\t@echo all\nx:\n\t@exit 5\n\t@echo x-after\n"
                      "y: x\n\t@echo y\nz:\n\t@echo z\nother:\n\t@echo other\n")
        self.expect(["-k", "all", "other"], "z\nother\n",
                    "minimake: *** [x] Error 5\nminimake: Target 'all' not remade because of errors.\n", 2)

    def test_keep_going_with_missing_rules(self):
        self.makefile("all: a missing b\n\t@echo all\na:\n\t@echo a\nb:\n\t@echo b\n")
        self.expect(["-k"], "a\nb\n",
                    "minimake: *** No rule to make target 'missing', needed by 'all'.\n"
                    "minimake: Target 'all' not remade because of errors.\n", 2)

    def test_keep_going_with_missing_goal(self):
        self.makefile("all:\n\t@echo all\n")
        self.expect(["-k", "nope", "all"], "all\n",
                    "minimake: *** No rule to make target 'nope'.\nminimake: Target 'nope' not remade because of errors.\n", 2)

    def test_keep_going_goal_recipe_fails(self):
        self.makefile("all:\n\tfalse\n\techo never\n")
        self.expect(["-k"], "false\n", "minimake: *** [all] Error 1\nminimake: Target 'all' not remade because of errors.\n", 2)

    def test_keep_going_failed_target_is_not_retried(self):
        self.makefile(".PHONY: a b bad\na: bad\n\t@echo a\nb: bad\n\t@echo b\nbad:\n\t@echo trying; exit 1\n")
        self.expect(["-k", "a", "b"], "trying\n",
                    "minimake: *** [bad] Error 1\nminimake: Target 'a' not remade because of errors.\n"
                    "minimake: Target 'b' not remade because of errors.\n", 2)

    def test_keep_going_success_exits_zero(self):
        self.makefile("all:\n\t@echo fine\n\t-false\n")
        self.expect(["-k"], "fine\nfalse\n", "minimake: [all] Error 1 (ignored)\n", 0)

    def test_failure_leaves_files_alone(self):
        self.makefile("t: s\n\techo partial > t; exit 1\n")
        self.files(s=OLD)
        self.expect([], "echo partial > t; exit 1\n", "minimake: *** [t] Error 1\n", 2)
        self.assertEqual((self.dir / "t").read_text(), "partial\n")


if __name__ == "__main__":
    unittest.main()
