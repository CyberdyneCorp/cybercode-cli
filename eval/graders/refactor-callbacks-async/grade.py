"""Hidden grader for refactor-callbacks-async.

Usage: python3 grade.py <workspace>
Prints `FAIL <reason>` lines and a final PASS or FAIL; exit 0 only on PASS.
The workspace is copied to a temporary directory and never modified.

1. Static rules on src/**/*.js: no `promisify`/`callbackify`, `new Promise` only in
   src/scheduler.js, and no function parameter named cb, callback, done or next.
2. The visible tests (rewritten for the promise API, titles kept) must pass.
3. hidden.test.mjs: randomized differential tests of the promise API against the frozen
   callback original in original/ (copied in as grader_original/), under a fake scheduler that
   records the order of every side effect.
"""

import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
TIMEOUT = 120
VISIBLE_TESTS = {"test/pipeline.test.js": ["pipeline transforms every record and writes a summary",
                                           "store reports missing files", "mapLimit keeps input order",
                                           "retry gives up after the configured attempts"]}
HIDDEN_TESTS = "hidden.test.mjs"
CALLBACK_NAMES = {"cb", "callback", "done", "next"}
CONTROL_KEYWORDS = ("if", "for", "while", "switch", "catch", "with", "return", "typeof", "await")
PARAM_LIST = re.compile(r"\(([^()]*)\)\s*(?:=>|\{)")
SINGLE_PARAM_ARROW = re.compile(r"(?<![\w$.])([\w$]+)\s*=>")
COMMENTS = re.compile(r"/\*.*?\*/|//[^\n]*", re.S)


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


def callback_params(code: str) -> set[str]:
    """Parameter names of functions, methods and arrows that look like completion callbacks."""
    names = set()
    for match in PARAM_LIST.finditer(code):
        before = code[:match.start()].rstrip()
        if re.search(r"\b(?:%s)$" % "|".join(CONTROL_KEYWORDS), before):
            continue
        for param in match.group(1).split(","):
            name = param.split("=")[0].strip().lstrip(".").strip()
            if name in CALLBACK_NAMES:
                names.add(name)
    names.update(n for n in SINGLE_PARAM_ARROW.findall(code) if n in CALLBACK_NAMES)
    return names


def static_problems(work: Path) -> list[str]:
    problems = []
    for path in sorted((work / "src").rglob("*.js")):
        rel = path.relative_to(work).as_posix()
        code = COMMENTS.sub("", path.read_text())
        if re.search(r"promisify|callbackify", code):
            problems.append(f"{rel} uses promisify/callbackify")
        if "new Promise" in code and rel != "src/scheduler.js":
            problems.append(f"{rel} uses `new Promise` (only src/scheduler.js may)")
        for name in sorted(callback_params(code)):
            problems.append(f"{rel} still has a function parameter named {name!r}")
    return problems


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
        problems = static_problems(work) + visible_test_problems(work)
        shutil.copytree(HERE / "original", work / "grader_original", dirs_exist_ok=True)
        shutil.copy(HERE / HIDDEN_TESTS, work)
        problems += [f"hidden tests: {f}" for f in node_test(work, HIDDEN_TESTS)]
    for problem in problems:
        print(f"FAIL {problem}")
    print("FAIL" if problems else "PASS")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
