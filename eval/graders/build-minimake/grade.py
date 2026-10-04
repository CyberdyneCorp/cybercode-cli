"""Hidden grader for build-minimake (long task).

Usage: python3 grade.py <workspace>
Prints `FAIL <reason>` lines and a final PASS or FAIL; exit 0 only on PASS.

Checks, on a temporary copy of the workspace (never the workspace itself):
  1. SPEC.md is unchanged;
  2. the agent's own tests in test/ run with `node --test`: at least MIN_AGENT_TESTS pass, none fail;
  3. README.md has a `## Usage` section with `node minimake.js` examples of -f, -C, -n, -k, -B and
     a NAME=VALUE override;
  4. the hidden behavior suite (hidden_test_minimake.py), which drives `node minimake.js` through
     subprocesses in temporary directories with explicit modification times, passes.
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
SPEC_SHA256 = "3e4210ae42fa96377922009c703d2f64fc4a2ceb9b117e3139f15280d5930af7"
HIDDEN_TESTS = "hidden_test_minimake"
USAGE_EXAMPLES = {  # what each README example must show -> regex on the text after `node minimake.js`
    "-f": r"\s-[nkB]*f", "-C": r"\s-[nkB]*C", "-n": r"\s-[kB]*n", "-k": r"\s-[nB]*k", "-B": r"\s-[nk]*B",
    "a NAME=VALUE override": r"\s[A-Za-z_][A-Za-z0-9_]*=",
}


def run(cmd: list[str], cwd: Path, **env: str) -> subprocess.CompletedProcess | None:
    environment = {k: v for k, v in os.environ.items() if k not in ("NODE_OPTIONS", "MAKEFLAGS")}
    environment.update(PYTHONDONTWRITEBYTECODE="1", **env)
    try:
        return subprocess.run(cmd, cwd=cwd, env=environment, capture_output=True, text=True, timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        return None


def spec_problems(work: Path) -> list[str]:
    spec = work / "SPEC.md"
    if not spec.exists() or hashlib.sha256(spec.read_bytes()).hexdigest() != SPEC_SHA256:
        return ["SPEC.md was modified or deleted"]
    return []


def agent_test_problems(work: Path) -> list[str]:
    if not (work / "minimake.js").exists():
        return ["minimake.js does not exist"]
    if not list((work / "test").rglob("*.*js")):
        return ["test/ has no test files"]
    result = run(["node", "--test", "--test-reporter=tap"], work)
    if result is None:
        return [f"agent tests: timed out after {TIMEOUT}s"]
    failed = re.findall(r"^\s*not ok \d+ - (.+)$", result.stdout, re.M)
    problems = [f"agent tests: {name}" for name in failed]
    passed = re.search(r"^# pass (\d+)", result.stdout, re.M)
    count = int(passed.group(1)) if passed else 0
    if result.returncode != 0 and not problems:
        problems.append(f"agent tests: exit {result.returncode}")
    if count < MIN_AGENT_TESTS:
        problems.append(f"agent tests: only {count} tests passed, expected at least {MIN_AGENT_TESTS}")
    return problems


def readme_problems(work: Path) -> list[str]:
    readme = work / "README.md"
    text = readme.read_text(encoding="utf-8") if readme.exists() else ""
    match = re.search(r"^## Usage\s*$(.*?)(?=^## |\Z)", text, re.M | re.S)
    if not match:
        return ["README.md has no `## Usage` section"]
    examples = re.findall(r"node minimake\.js(\s.*)", match.group(1))
    return [f"README.md usage has no `node minimake.js` example of {name}" for name, pattern in USAGE_EXAMPLES.items()
            if not any(re.search(pattern, example) for example in examples)]


def hidden_test_problems(work: Path, tmp: Path) -> list[str]:
    hidden_dir = tmp / "hidden"
    hidden_dir.mkdir()
    shutil.copy(HERE / f"{HIDDEN_TESTS}.py", hidden_dir)
    result = run([sys.executable, "-m", "unittest", "-v", HIDDEN_TESTS], hidden_dir, MINIMAKE_WORKSPACE=str(work))
    if result is None:
        return [f"hidden tests: timed out after {TIMEOUT}s"]
    if result.returncode == 0:
        return []
    failed = [line for line in result.stderr.splitlines() if line.startswith(("FAIL: ", "ERROR: "))]
    return [f"hidden tests: {line}" for line in failed] or [f"hidden tests: exit {result.returncode}"]


def main() -> int:
    workspace = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp) / "ws"
        shutil.copytree(workspace, work, ignore=shutil.ignore_patterns(".git", "node_modules", "__pycache__"))
        problems = spec_problems(work) + agent_test_problems(work) + readme_problems(work)
        problems += hidden_test_problems(work, Path(tmp))
    for problem in problems:
        print(f"FAIL {problem}")
    print("FAIL" if problems else "PASS")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
