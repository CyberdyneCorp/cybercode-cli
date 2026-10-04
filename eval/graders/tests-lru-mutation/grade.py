"""Hidden grader for tests-lru-mutation (mutation testing of the agent's tests).

Usage: python3 grade.py <workspace>
Prints `FAIL <reason>` lines and a final PASS or FAIL; exit 0 only on PASS.

The agent's `node --test` suite must pass against the reference implementation and fail
against every mutant in mutants/, each of which breaks one clause of the README contract.
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
IMPLEMENTATION = "src/lruCache.js"
# Tests must exercise behavior, not fingerprint the implementation's source.
FORBIDDEN = re.compile(r"readFile|createHash|node:fs|from \"fs\"|from 'fs'|\.toString\(\)\s*\.\s*includes")


def run_tests(work: Path) -> tuple[str, int]:
    """Run the agent's suite; return (status, tests_ran) with status passed/failed/timeout."""
    cmd = ["node", "--test", "--test-reporter=tap"]
    try:
        result = subprocess.run(cmd, cwd=work, capture_output=True, text=True, timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        return "timeout", 0
    ran = re.search(r"^# tests (\d+)", result.stdout, re.M)
    return ("passed" if result.returncode == 0 else "failed"), int(ran.group(1)) if ran else 0


def static_problems(workspace: Path) -> list[str]:
    tests = list((workspace / "test").glob("**/*.test.js")) if (workspace / "test").is_dir() else []
    if not tests:
        return ["no test/**/*.test.js files"]
    problems = []
    if (workspace / IMPLEMENTATION).read_bytes() != (HERE / "reference" / "lruCache.js").read_bytes():
        problems.append(f"{IMPLEMENTATION} was modified")
    for path in tests:
        if FORBIDDEN.search(path.read_text()):
            problems.append(f"{path.name} inspects the implementation source instead of its behavior")
    return problems


def mutation_problems(workspace: Path) -> list[str]:
    problems = []
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp) / "ws"
        shutil.copytree(workspace, work, ignore=shutil.ignore_patterns(".git", "node_modules"))
        shutil.copy(HERE / "reference" / "lruCache.js", work / IMPLEMENTATION)
        status, ran = run_tests(work)
        if status != "passed":
            return [f"the tests do not pass against the correct implementation ({status})"]
        if ran == 0:
            return ["no tests ran"]
        for mutant in sorted((HERE / "mutants").glob("*.js")):
            shutil.copy(mutant, work / IMPLEMENTATION)
            status, _ = run_tests(work)
            if status == "passed":
                problems.append(f"mutant survived: {mutant.stem} ({mutant.read_text().splitlines()[0][3:]})")
    return problems


def main() -> int:
    workspace = Path(sys.argv[1]).resolve()
    problems = static_problems(workspace)
    if not problems:
        problems = mutation_problems(workspace)
    for problem in problems:
        print(f"FAIL {problem}")
    print("FAIL" if problems else "PASS")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
