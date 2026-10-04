"""Hidden grader for perf-leaderboard.

Usage: python3 grade.py <workspace>
Prints `FAIL <reason>` lines and a final PASS or FAIL; exit 0 only on PASS.
The workspace is copied to a temporary directory and never modified.

1. The visible tests must still exist and pass.
2. hidden.test.mjs: randomized differential tests of Leaderboard, service and CLI against the
   frozen original in original/ (copied in as grader_original/).
3. hidden_bench.mjs: three 100k-player / 200k-operation workloads timed against LIMIT_SECONDS
   each, with results checked against the reference treap in reference/. The reference takes
   about 0.7 s per workload on the grading machine; the original does not finish at all.
"""

import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
TIMEOUT = 120
BENCH_TIMEOUT = 30
LIMIT_SECONDS = 4.0
WORKLOADS = ["mixed", "heavy ties", "increasing scores"]
VISIBLE_TESTS = {
    "test/leaderboard.test.js": ["tied players share a rank", "update sets the score and returns the previous one",
                                 "topK orders ties by name", "around, countInRange and percentile"],
    "test/service.test.js": ["submit keeps the personal best", "closeSeason stores the podium and clears the board"],
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


def benchmark_problems(work: Path) -> list[str]:
    try:
        result = subprocess.run(["node", "hidden_bench.mjs"], cwd=work, capture_output=True, text=True,
                                timeout=BENCH_TIMEOUT)
        stdout = result.stdout
    except subprocess.TimeoutExpired as expired:
        stdout = expired.stdout.decode() if isinstance(expired.stdout, bytes) else (expired.stdout or "")
    runs = {}
    for line in stdout.splitlines():
        if line.startswith("{"):
            record = json.loads(line)
            runs[record["name"]] = record
    problems = []
    for name in WORKLOADS:
        record = runs.get(name)
        if record is None:
            problems.append(f"benchmark {name!r}: did not finish (benchmark process limit {BENCH_TIMEOUT}s)")
        elif record["checksum"] != record["expected"]:
            problems.append(f"benchmark {name!r}: results differ from the reference")
        elif record["seconds"] > LIMIT_SECONDS:
            problems.append(f"benchmark {name!r}: {record['seconds']:.2f}s exceeds the {LIMIT_SECONDS}s limit")
    return problems


def main() -> int:
    workspace = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp) / "ws"
        shutil.copytree(workspace, work, ignore=shutil.ignore_patterns(".git", "node_modules"))
        problems = visible_test_problems(work)
        shutil.copytree(HERE / "original", work / "grader_original", dirs_exist_ok=True)
        shutil.copytree(HERE / "reference", work / "grader_reference", dirs_exist_ok=True)
        shutil.copy(HERE / HIDDEN_TESTS, work)
        shutil.copy(HERE / "hidden_bench.mjs", work)
        problems += [f"hidden tests: {f}" for f in node_test(work, HIDDEN_TESTS)]
        problems += benchmark_problems(work)
    for problem in problems:
        print(f"FAIL {problem}")
    print("FAIL" if problems else "PASS")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
