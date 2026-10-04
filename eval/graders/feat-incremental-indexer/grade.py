"""Hidden grader for feat-incremental-indexer.

Lives outside the fixture so the agent cannot read or edit it. Usage:
    python3 grade.py <workspace>
Prints `FAIL <reason>` lines and a final PASS or FAIL; exit 0 only on PASS.
The workspace is copied to a temporary directory and never modified.

The hidden suite (hidden_test_incremental.py) checks every build against a full build by a
frozen reference implementation (reference/refindexer): identical index bytes and the exact
IndexStats counts from docs/INCREMENTAL.md, on targeted cases and 40 seeded random
sequences of file system mutations (with explicit os.utime mtimes) and config changes.
"""

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
TIMEOUT = 300
VISIBLE_TESTS = {
    "tests/test_build.py": ["test_full_build_and_search", "test_index_file_is_canonical_json"],
    "tests/test_query.py": ["test_and_or", "test_phrase_respects_stopword_gaps", "test_syntax_errors"],
    "tests/test_tokenizer.py": ["test_positions_skip_stopwords", "test_stemming"],
}
HIDDEN_TESTS = "hidden_test_incremental"


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
        shutil.copytree(HERE / "reference", work / "_grader_ref", dirs_exist_ok=True)
        result = run([sys.executable, "-m", "unittest", "-v", HIDDEN_TESTS], work)
        problems += unittest_problems("hidden tests", result)
    for problem in problems:
        print(f"FAIL {problem}")
    print("FAIL" if problems else "PASS")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
