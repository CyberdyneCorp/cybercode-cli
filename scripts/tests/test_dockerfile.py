"""The repository `Dockerfile` (`installation-upgrade` → Container and CI usage). The image
is not built here; hadolint lints the file when it is installed."""

import re
import shutil
import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import release_assets as ra  # noqa: E402

DOCKERFILE = ra.ROOT / "Dockerfile"


class Dockerfile(unittest.TestCase):
    def setUp(self):
        self.text = DOCKERFILE.read_text()

    def test_base_image_is_pinned_by_digest(self):
        self.assertRegex(self.text, r"(?m)^FROM debian:bookworm-slim@sha256:[0-9a-f]{64}$")

    def test_packages(self):
        install = re.search(r"apt-get install[^\n]*", self.text)[0]
        for package in ["ca-certificates", "git", "ripgrep", "bubblewrap"]:
            self.assertRegex(install, rf"\b{package}\b")

    def test_runs_as_cyber_10001(self):
        self.assertIn("groupadd --gid 10001 cyber", self.text)
        self.assertIn("useradd --uid 10001 --gid 10001", self.text)
        self.assertRegex(self.text, r"(?m)^USER cyber$")

    def test_environment_and_entrypoint(self):
        self.assertIn("CYBER_HOME=/home/cyber/.cyber", self.text)
        self.assertRegex(self.text, r"\bCI=true\b")
        self.assertRegex(self.text, r"(?m)^WORKDIR /work$")
        self.assertRegex(self.text, r'(?m)^ENTRYPOINT \["cyber"\]$')

    def test_copies_the_prebuilt_binaries(self):
        self.assertIn("ARG TARGETARCH", self.text)
        self.assertIn("dist/${TARGETARCH}/cyber ", self.text)
        self.assertIn("dist/${TARGETARCH}/cyber-sandbox-exec", self.text)
        ignore = (ra.ROOT / ".dockerignore").read_text().split()
        self.assertIn("*", ignore)
        self.assertIn("!dist/", ignore)

    @unittest.skipUnless(shutil.which("hadolint"), "hadolint is not installed")
    def test_hadolint(self):
        subprocess.run(["hadolint", str(DOCKERFILE)], check=True)


if __name__ == "__main__":
    unittest.main()
