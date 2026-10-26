"""Independent real-repository grader; original workspace is never changed."""

from pathlib import Path
import os, shutil, subprocess, sys, tempfile, json, hashlib, re

HERE = Path(__file__).resolve().parent


def main():
    source = Path(sys.argv[1]).resolve()
    if not (source / "Cargo.toml").is_file() or not (source / "Cargo.lock").is_file():
        print("FAIL required repository manifest/lock removed")
        print("FAIL")
        return 1
    for name, expected in json.loads((HERE / "constraints.json").read_text()).items():
        path = source / name
        if (
            not path.is_file()
            or hashlib.sha256(path.read_bytes()).hexdigest() != expected
        ):
            print("FAIL dependency manifest or lock changed")
            print("FAIL")
            return 1
    with tempfile.TemporaryDirectory(prefix="cyber-rust-grade-") as temporary:
        work = Path(temporary) / "repo"
        shutil.copytree(
            source,
            work,
            symlinks=True,
            ignore=shutil.ignore_patterns(".git", "target", "__pycache__"),
        )
        package = "globset"
        tests = work / "crates" / package / "tests"
        tests.mkdir(exist_ok=True)
        shutil.copyfile(HERE / "hidden.rs", tests / "cyber_eval_hidden.rs")
        env = dict(
            os.environ,
            CARGO_INCREMENTAL="0",
            CARGO_PROFILE_DEV_DEBUG="0",
            CARGO_PROFILE_TEST_DEBUG="0",
            CARGO_NET_OFFLINE="true",
        )
        command = [
            "cargo",
            "test",
            "--manifest-path",
            str(work / "Cargo.toml"),
            "--locked",
            "--offline",
            "-p",
            package,
            "--lib",
            "--test",
            "cyber_eval_hidden",
        ]
        p = subprocess.run(
            command,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=240,
        )
        print(p.stdout[-14000:])
        if p.returncode and any(
            s in p.stdout
            for s in [
                "failed to download",
                "no matching package named",
                "attempting to make an HTTP request",
                "could not execute process",
            ]
        ):
            print("INFRA offline dependency/toolchain setup unavailable")
            return 2
        tail = p.stdout.split("Running tests/cyber_eval_hidden.rs")[-1]
        passed = (
            p.returncode == 0
            and "Running tests/cyber_eval_hidden.rs" in p.stdout
            and re.search(r"test result: ok\. 1 passed; 0 failed;", tail) is not None
        )
        print("PASS" if passed else "FAIL")
        return 0 if passed else 1


try:
    sys.exit(main())
except (OSError, subprocess.TimeoutExpired) as e:
    print("INFRA " + type(e).__name__)
    sys.exit(2)
