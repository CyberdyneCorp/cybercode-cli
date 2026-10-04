"""Hidden grader for fix-cursor-pagination.

Runs the visible tests (they must still exist and pass) and hidden.test.mjs: parameter
validation, cursor binding and decoding errors, the README ordering (code units, nulls last in
both directions, id tie-break), seeded forward and backward walks for every sort and page size
over datasets with heavy ties and nulls, and seeded walks with inserts and deletes between pages
checked against a reference model.

Usage: python3 grade.py <workspace>
Prints `FAIL <reason>` lines and a final PASS or FAIL; exit 0 only on PASS.
The workspace is copied to a temporary directory and never modified.
"""

import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
TIMEOUT = 60
VISIBLE_TESTS = {
    "test/listItems.test.js": ["walks all items in updatedAt order", "walks items with equal prices exactly once"],
}
HIDDEN_TESTS = "hidden.test.mjs"


def node_test(work: Path, *files: str) -> list[str]:
    """Run `node --test` and return one line per failing test (empty when all pass)."""
    cmd = ["node", "--test", "--test-reporter=tap", *files]
    try:
        result = subprocess.run(cmd, cwd=work, capture_output=True, text=True, timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        return [f"timed out after {TIMEOUT}s"]
    if result.returncode == 0:
        return []
    failed = re.findall(r"^\s*not ok \d+ - (.+)$", result.stdout, re.M)
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


def main() -> int:
    workspace = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp) / "ws"
        shutil.copytree(workspace, work, ignore=shutil.ignore_patterns(".git", "__pycache__", "node_modules"))
        problems = visible_test_problems(work)
        shutil.copy(HERE / HIDDEN_TESTS, work)
        problems += [f"hidden tests: {f}" for f in node_test(work, HIDDEN_TESTS)]
    for problem in problems:
        print(f"FAIL {problem}")
    print("FAIL" if problems else "PASS")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
