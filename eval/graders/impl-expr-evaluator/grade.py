"""Hidden grader for impl-expr-evaluator.

Usage: python3 grade.py <workspace>
Prints `FAIL <reason>` lines and a final PASS or FAIL; exit 0 only on PASS.

The visible tests must still exist and pass; then hidden_test_calc.py (several hundred
table-driven cases, each a value or an exact error message and column from SPEC.md) runs
against the workspace's `calc` package. The workspace is copied to a temporary directory and
never modified.
"""

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
TIMEOUT = 60
VISIBLE_TESTS = {"test_calc.py": ["test_precedence", "test_division_gives_decimal", "test_strings",
                                  "test_variables_and_let", "test_division_by_zero"]}
HIDDEN_TESTS = "hidden_test_calc"
MAX_REPORTED = 40


def run(cmd: list[str], cwd: Path) -> subprocess.CompletedProcess | None:
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
    try:
        return subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True, timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        return None


def unittest_problems(label: str, result: subprocess.CompletedProcess | None) -> list[str]:
    if result is None:
        return [f"{label}: timed out after {TIMEOUT}s"]
    if result.returncode == 0:
        return []
    failed = [line for line in result.stderr.splitlines() if line.startswith(("FAIL: ", "ERROR: "))]
    if len(failed) > MAX_REPORTED:
        failed = failed[:MAX_REPORTED] + [f"... and {len(failed) - MAX_REPORTED} more failing cases"]
    return [f"{label}: {line}" for line in failed] or [f"{label}: exit {result.returncode}"]


def visible_test_problems(work: Path) -> list[str]:
    problems = []
    for name, tests in VISIBLE_TESTS.items():
        path = work / name
        if not path.exists():
            problems.append(f"visible test file {name} was deleted")
            continue
        text = path.read_text()
        problems += [f"visible test {name}::{t} was removed" for t in tests if f"def {t}(" not in text]
    result = run([sys.executable, "-m", "unittest", "-v"], work)
    return problems + unittest_problems("visible tests", result)


def main() -> int:
    workspace = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp) / "ws"
        shutil.copytree(workspace, work, ignore=shutil.ignore_patterns(".git", "__pycache__"))
        problems = visible_test_problems(work)
        shutil.copy(HERE / f"{HIDDEN_TESTS}.py", work)
        problems += unittest_problems("hidden tests", run([sys.executable, "-m", "unittest", HIDDEN_TESTS], work))
    for problem in problems:
        print(f"FAIL {problem}")
    print("FAIL" if problems else "PASS")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
