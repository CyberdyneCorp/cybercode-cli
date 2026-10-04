"""Hidden grader for refactor-exporter-registry.

Usage: python3 grade.py <workspace>
Prints `FAIL <reason>` lines and a final PASS or FAIL; exit 0 only on PASS.

Checks, on a temporary copy of the workspace (never the workspace itself):
  1. the visible tests still exist and pass;
  2. hidden_test_diff.py: randomized differential tests of every library function and many CLI
     invocations against the original package (frozen in original_reportx/), byte for byte;
  3. hidden_test_structure.py: the registry API in reportx/exporters/, one module per built-in
     format, and format names/aliases/extensions/content types only in their exporter module;
  4. hidden_test_plugin.py: after dropping plugin/yaml_lines.py into reportx/exporters/, the new
     format works in the library, the HTTP headers, file names and the CLI.
"""

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
TIMEOUT = 180
VISIBLE_TESTS = {"test_reportx.py": ["test_csv", "test_markdown_alias", "test_ndjson_has_no_trailing_newline",
                                     "test_metadata", "test_writes_into_output_dir"]}
MAX_REPORTED = 30


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
    return [f"{label}: {line[:300]}" for line in failed] or [f"{label}: exit {result.returncode}"]


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


def hidden(work: Path, module: str) -> list[str]:
    shutil.copy(HERE / f"{module}.py", work)
    return unittest_problems(module, run([sys.executable, "-m", "unittest", module], work))


def main() -> int:
    workspace = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp) / "ws"
        shutil.copytree(workspace, work, ignore=shutil.ignore_patterns(".git", "__pycache__"))
        problems = visible_test_problems(work)
        shutil.copytree(HERE / "original_reportx", work / "original_reportx", ignore=shutil.ignore_patterns("__pycache__"))
        problems += hidden(work, "hidden_test_diff")
        if (work / "reportx" / "exporters").is_dir():
            problems += hidden(work, "hidden_test_structure")
            shutil.copy(HERE / "plugin" / "yaml_lines.py", work / "reportx" / "exporters" / "yaml_lines.py")
            problems += hidden(work, "hidden_test_plugin")
        else:
            problems.append("reportx/exporters/ does not exist")
    for problem in problems:
        print(f"FAIL {problem}")
    print("FAIL" if problems else "PASS")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
