"""The npm launcher `packaging/npm/cli/bin/cyber.js` (`installation-upgrade` → Package
channels "npm launcher"): run with node against a stub platform binary laid out as npm
installs the rendered packages."""

import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import release_assets as ra  # noqa: E402

NODE = shutil.which("node")

# Prints its arguments, then exits with the code in $STUB_EXIT, or waits for SIGINT or
# SIGTERM and reports it when $STUB_WAIT is set.
STUB = """#!/bin/sh
for arg in "$@"; do printf '[%s]\\n' "$arg"; done
if [ -n "${STUB_WAIT:-}" ]; then
    trap 'echo got-int > "$STUB_WAIT"; exit 6' INT
    trap 'echo got-term > "$STUB_WAIT"; exit 7' TERM
    echo ready > "$STUB_WAIT.ready"
    while :; do sleep 0.05; done
fi
exit "${STUB_EXIT:-0}"
"""


def node_key() -> str:
    out = subprocess.run(
        [NODE, "-p", "process.platform + '-' + process.arch"],
        check=True, capture_output=True, text=True,
    )
    return out.stdout.strip()


@unittest.skipUnless(NODE and os.name == "posix", "node and a POSIX shell are needed")
class NpmLauncher(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        key = node_key()
        values = {"version": "1.4.0", "npm.key": key}
        values["npm.os"], values["npm.cpu"] = key.split("-")
        modules = self.tmp / "node_modules" / "@cyber-code"
        cli = modules / "cli" / "bin"
        cli.mkdir(parents=True)
        shutil.copy(ra.ROOT / "packaging/npm/cli/bin/cyber.js", cli / "cyber.js")
        platform = modules / f"cli-{key}"
        (platform / "bin").mkdir(parents=True)
        template = (ra.ROOT / "packaging/npm/platform/package.json").read_text()
        rendered = ra.render(template, values)
        self.assertEqual(json.loads(rendered)["name"], f"@cyber-code/cli-{key}")
        (platform / "package.json").write_text(rendered)
        stub = platform / "bin" / "cyber"
        stub.write_text(STUB)
        stub.chmod(0o755)
        self.launcher = cli / "cyber.js"

    def run_launcher(self, args, **env):
        return subprocess.run(
            [NODE, str(self.launcher), *args],
            capture_output=True, text=True, env={**os.environ, **env}, timeout=30,
        )

    def test_forwards_arguments_and_exit_code(self):
        result = self.run_launcher(["exec", "hello world", "--format", "json"], STUB_EXIT="3")
        self.assertEqual(result.returncode, 3, result.stderr)
        self.assertEqual(result.stdout, "[exec]\n[hello world]\n[--format]\n[json]\n")

    def test_forwards_sigint_and_sigterm(self):
        for sig, code, name in [(signal.SIGINT, 6, "int"), (signal.SIGTERM, 7, "term")]:
            marker = self.tmp / f"signal-{name}"
            child = subprocess.Popen(
                [NODE, str(self.launcher)], env={**os.environ, "STUB_WAIT": str(marker)}
            )
            deadline = time.monotonic() + 20
            while not Path(f"{marker}.ready").exists():
                self.assertLess(time.monotonic(), deadline, "the stub never started")
                time.sleep(0.05)
            child.send_signal(sig)
            self.assertEqual(child.wait(timeout=20), code, name)
            self.assertEqual(marker.read_text(), f"got-{name}\n")

    def test_missing_platform_package_exits_1(self):
        shutil.rmtree(self.tmp / "node_modules" / "@cyber-code" / f"cli-{node_key()}")
        result = self.run_launcher(["--version"])
        self.assertEqual(result.returncode, 1)
        self.assertIn("is not installed", result.stderr)


if __name__ == "__main__":
    unittest.main()
