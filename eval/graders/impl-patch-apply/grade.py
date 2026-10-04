"""Hidden grader for impl-patch-apply.

Usage: python3 grade.py <workspace>
Prints `FAIL <reason>` lines and a final PASS or FAIL; exit 0 only on PASS.
The workspace is copied to a temporary directory and never modified.

Hidden tests: table-driven edge cases for every rule of SPEC.md (format, markers, CRLF,
create/delete, exact error messages, offset search order, maxOffset, ordering, atomicity,
reverse) plus seeded randomized patches produced by an LCS diff, which must reproduce the
target exactly and, reversed, restore the source.
"""

import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
TIMEOUT = 120
VISIBLE_TESTS = {"test/applyPatch.test.js": ["applies a single hunk", "reports a failing hunk"]}
PROTECTED = ["SPEC.md"]
HIDDEN_TESTS = "hidden.test.mjs"
MAX_REPORTED = 25


def node_test(work: Path, *files: str) -> list[str]:
    """Run `node --test` and return one line per failing test (empty when all pass)."""
    cmd = ["node", "--test", "--test-reporter=tap", *files]
    try:
        result = subprocess.run(cmd, cwd=work, capture_output=True, text=True, timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        return [f"timed out after {TIMEOUT}s"]
    if result.returncode == 0:
        return []
    failed = re.findall(r"^not ok \d+ - (.+)$", result.stdout, re.M)
    return failed or [f"exit {result.returncode}: {result.stderr.strip()[-300:]}"]


def visible_test_problems(work: Path) -> list[str]:
    problems = []
    for name, tests in VISIBLE_TESTS.items():
        path = work / name
        if not path.exists():
            problems.append(f"visible test file {name} was deleted")
            continue
        text = path.read_text()
        problems += [f"visible test {name}: {t!r} was removed" for t in tests if t not in text]
    return problems + [f"visible tests: {f}" for f in node_test(work)]


def protected_problems(work: Path) -> list[str]:
    return [f"{name} was modified" for name in PROTECTED
            if not (work / name).exists() or (work / name).read_bytes() != (HERE / "frozen" / name).read_bytes()]


def main() -> int:
    workspace = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp) / "ws"
        shutil.copytree(workspace, work, ignore=shutil.ignore_patterns(".git", "node_modules"))
        problems = protected_problems(work) + visible_test_problems(work)
        shutil.copy(HERE / HIDDEN_TESTS, work)
        hidden = node_test(work, HIDDEN_TESTS)
        problems += [f"hidden tests: {f}" for f in hidden[:MAX_REPORTED]]
        if len(hidden) > MAX_REPORTED:
            problems.append(f"hidden tests: ... and {len(hidden) - MAX_REPORTED} more")
    for problem in problems:
        print(f"FAIL {problem}")
    print("FAIL" if problems else "PASS")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
