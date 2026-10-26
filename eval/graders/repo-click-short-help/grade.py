"""Frozen independent upstream cases run against candidate source, without its config."""

from pathlib import Path
import os, shutil, subprocess, sys, tempfile, re

HERE = Path(__file__).resolve().parent


def main():
    source = Path(sys.argv[1]).resolve()
    if (
        not (source / "src/click/__init__.py").is_file()
        or not (source / "tests/test_utils/test_make_default_short_help.py").is_file()
    ):
        print("FAIL required source or visible tests removed")
        print("FAIL")
        return 1
    with tempfile.TemporaryDirectory(prefix="cyber-click-grade-") as temporary:
        work = Path(temporary) / "repo"
        shutil.copytree(
            source,
            work,
            symlinks=True,
            ignore=shutil.ignore_patterns(".git", "__pycache__", ".pytest_cache"),
        )
        program = "import sys,pytest;sys.path.insert(0,sys.argv[1]);raise SystemExit(pytest.main(sys.argv[2:]))"
        args = [
            sys.executable,
            "-I",
            "-c",
            program,
            str(work / "src"),
            "-q",
            "-c",
            os.devnull,
            "--confcutdir",
            str(HERE / "upstream"),
            str(HERE / "upstream" / "test_utils/test_make_default_short_help.py"),
        ]
        env = dict(
            os.environ, PYTEST_DISABLE_PLUGIN_AUTOLOAD="1", PYTHONDONTWRITEBYTECODE="1"
        )
        p = subprocess.run(
            args,
            env=env,
            cwd=temporary,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=120,
        )
        passed = (
            p.returncode == 0
            and re.search(r"\b[1-9][0-9]* passed", p.stdout) is not None
        )
        print(p.stdout[-14000:])
        print("PASS" if passed else "FAIL")
        return 0 if passed else 1


try:
    sys.exit(main())
except (OSError, subprocess.TimeoutExpired) as e:
    print("INFRA " + type(e).__name__)
    sys.exit(2)
