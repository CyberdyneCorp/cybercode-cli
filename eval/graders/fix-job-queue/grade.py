"""Hidden grader for fix-job-queue.

Usage: python3 grade.py <workspace>
Prints `FAIL <reason>` lines and a final PASS or FAIL; exit 0 only on PASS.
The workspace is copied to a temporary directory and never modified.

The visible tests must still exist and pass; hidden.test.mjs adds targeted tests for every
README clause and 400 randomized scenarios on a fake clock that check the invariants
(concurrency, priority/FIFO order, retries, cancellation, pause/resume, drain, stats, events).
"""

import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
TIMEOUT = 120
VISIBLE_TESTS = {"test/queue.test.js": ["runs jobs by priority", "retries with exponential backoff",
                                        "drain resolves when the queue is empty",
                                        "a task that throws synchronously frees its slot"]}
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
    failures = []
    for block in re.split(r"^\s*(?=not ok \d+ - )", result.stdout, flags=re.M):
        name = re.match(r"not ok \d+ - (.+)", block)
        if name:
            error = re.search(r"error: (?:\|-?\s*\n\s*(.+)|'(.+)')", block)
            failures.append(name.group(1) + (f" ({error.group(1) or error.group(2)})" if error else ""))
    return failures or [f"exit {result.returncode}: {result.stderr.strip()[-300:]}"]


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
        shutil.copytree(workspace, work, ignore=shutil.ignore_patterns(".git", "node_modules"))
        problems = visible_test_problems(work)
        shutil.copy(HERE / HIDDEN_TESTS, work)
        problems += [f"hidden tests: {f}" for f in node_test(work, HIDDEN_TESTS)]
    for problem in problems:
        print(f"FAIL {problem}")
    print("FAIL" if problems else "PASS")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
