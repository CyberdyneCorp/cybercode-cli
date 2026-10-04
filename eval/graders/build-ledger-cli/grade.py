"""Hidden grader for build-ledger-cli (long task).

Usage: python3 grade.py <workspace>
Prints `FAIL <reason>` lines and a final PASS or FAIL; exit 0 only on PASS.

Checks, on a temporary copy of the workspace (never the workspace itself):
  1. the agent's own unittest suite in tests/ exists, runs at least MIN_AGENT_TESTS tests and passes;
  2. README.md has a `## Usage` section showing every command;
  3. SPEC.md is unchanged;
  4. the hidden behavior suite (hidden_test_ledger.py) passes: explicit cases with exact expected
     exit code/stdout/stderr (ledger_cases.py), argparse usage errors, and seeded random journals
     compared with the frozen reference implementation in reference/.
"""

import hashlib
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
TIMEOUT = 600
MIN_AGENT_TESTS = 20
COMMANDS = ["balance", "register", "print", "accounts", "payees", "prices", "stats", "check"]
HIDDEN_TESTS = "hidden_test_ledger"
SPEC_SHA256 = "d2397dd7af35833448e6aad1e940b67365079f6d2775cb13e9bfe2c2e0c67146"


def run(cmd: list[str], cwd: Path, **env: str) -> subprocess.CompletedProcess | None:
    environment = dict(os.environ, PYTHONDONTWRITEBYTECODE="1", PYTHONIOENCODING="utf-8", PYTHONUTF8="1", **env)
    try:
        return subprocess.run(cmd, cwd=cwd, env=environment, capture_output=True, text=True,
                              encoding="utf-8", errors="replace", timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        return None


def unittest_problems(label: str, result: subprocess.CompletedProcess | None) -> list[str]:
    if result is None:
        return [f"{label}: timed out after {TIMEOUT}s"]
    if result.returncode == 0:
        return []
    failed = [line for line in result.stderr.splitlines() if line.startswith(("FAIL: ", "ERROR: "))]
    return [f"{label}: {line}" for line in failed] or [f"{label}: exit {result.returncode}: {result.stderr[-300:]}"]


def agent_test_problems(work: Path) -> list[str]:
    if not (work / "ledger" / "__main__.py").exists():
        return ["ledger/__main__.py does not exist"]
    if not list((work / "tests").glob("test*.py")):
        return ["tests/ has no test*.py files"]
    result = run([sys.executable, "-m", "unittest", "discover", "-s", "tests"], work)
    problems = unittest_problems("agent tests", result)
    ran = re.search(r"^Ran (\d+) tests?", result.stderr, re.M) if result else None
    count = int(ran.group(1)) if ran else 0
    if count < MIN_AGENT_TESTS:
        problems.append(f"agent tests: only {count} tests ran, expected at least {MIN_AGENT_TESTS}")
    return problems


def readme_problems(work: Path) -> list[str]:
    readme = work / "README.md"
    text = readme.read_text(encoding="utf-8") if readme.exists() else ""
    match = re.search(r"^## Usage\s*$(.*?)(?=^## |\Z)", text, re.M | re.S)
    if not match:
        return ["README.md has no `## Usage` section"]
    usage = match.group(1)
    return [f"README.md usage does not show `{c}`" for c in COMMANDS
            if not re.search(rf"python3 -m ledger -f \S+ (--strict )?{c}\b", usage)]


def spec_problems(work: Path) -> list[str]:
    spec = work / "SPEC.md"
    if not spec.exists() or hashlib.sha256(spec.read_bytes()).hexdigest() != SPEC_SHA256:
        return ["SPEC.md was modified or deleted"]
    return []


def main() -> int:
    workspace = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp) / "ws"
        shutil.copytree(workspace, work, ignore=shutil.ignore_patterns(".git", "__pycache__"))
        problems = spec_problems(work) + agent_test_problems(work) + readme_problems(work)
        hidden_dir = Path(tmp) / "hidden"
        shutil.copytree(HERE, hidden_dir, ignore=shutil.ignore_patterns("__pycache__", "grade.py"))
        result = run([sys.executable, "-m", "unittest", HIDDEN_TESTS], hidden_dir,
                     LEDGER_WORKSPACE=str(work), LEDGER_REFERENCE=str(hidden_dir / "reference"))
        problems += unittest_problems("hidden tests", result)
    for problem in problems:
        print(f"FAIL {problem}")
    print("FAIL" if problems else "PASS")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
