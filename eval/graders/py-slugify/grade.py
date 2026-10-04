"""Hidden grader for the py-slugify fixture.

Lives outside the fixture so the agent cannot read or edit it. Usage:
    python3 grade.py <workspace>
Exit 0 when every hidden assertion and the visible test file pass.
"""

import importlib.util
import pathlib
import subprocess
import sys

CASES = {
    "Hello": "hello",
    "Hello,  World!": "hello-world",
    "  leading and trailing  ": "leading-and-trailing",
    "Rust 2024 Edition": "rust-2024-edition",
    "---": "",
    "Ünïcode Títle": "ünïcode-títle",
}


def load(workspace: pathlib.Path):
    spec = importlib.util.spec_from_file_location("slugify", workspace / "slugify.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.slugify


def main() -> int:
    workspace = pathlib.Path(sys.argv[1]).resolve()
    if not (workspace / "test_slugify.py").exists():
        print("FAIL visible tests were deleted")
        return 1
    slugify = load(workspace)
    failures = [(i, e, slugify(i)) for i, e in CASES.items() if slugify(i) != e]
    for given, expected, actual in failures:
        print(f"FAIL slugify({given!r}) = {actual!r}, expected {expected!r}")
    visible = subprocess.run([sys.executable, "-m", "unittest", "-q"], cwd=workspace)
    if visible.returncode != 0:
        print("FAIL visible tests")
    ok = not failures and visible.returncode == 0
    print("PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
