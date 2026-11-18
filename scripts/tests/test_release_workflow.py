"""`.github/workflows/release.yml` (`installation-upgrade` → Release targets "Not on pull
requests"), checked as text: YAML 1.1 parsers read the `on` key as a boolean."""

import json
import re
import shutil
import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import release_assets as ra  # noqa: E402

WORKFLOW = ra.ROOT / ".github" / "workflows" / "release.yml"
CI = ra.ROOT / ".github" / "workflows" / "ci.yml"
BUILD_TARGETS = ra.ROOT / ".github" / "build-targets.json"


def build_targets() -> list[dict]:
    """The CI build matrix (`ci.yml` → platforms), before the pull-request filter."""
    return json.loads(BUILD_TARGETS.read_text())


def top_level_block(text: str, key: str) -> str:
    """The lines of the top-level `key:` block, up to the next top-level key."""
    match = re.search(rf"^{key}:\n((?: .*\n|\n)*)", text, re.MULTILINE)
    assert match, f"no top-level {key}"
    return match[1].rstrip("\n") + "\n"


def jobs(text: str) -> dict[str, str]:
    body = top_level_block(text, "jobs")
    parts = re.split(r"^  ([a-z][a-z0-9_-]*):\n", body, flags=re.MULTILINE)
    return dict(zip(parts[1::2], parts[2::2]))


class ReleaseWorkflow(unittest.TestCase):
    def setUp(self):
        self.text = WORKFLOW.read_text()

    def test_only_a_version_tag_push_triggers_it(self):
        trigger = top_level_block(self.text, "on")
        self.assertEqual(trigger, '  push:\n    tags:\n      - "v*"\n')
        for event in ["pull_request", "workflow_dispatch", "branches", "schedule"]:
            self.assertNotIn(event, trigger)

    def test_only_publish_can_write_or_sign(self):
        self.assertEqual(top_level_block(self.text, "permissions"), "  contents: read\n")
        per_job = jobs(self.text)
        self.assertEqual(set(per_job), {"verify", "build", "extension", "publish"})
        for name, body in per_job.items():
            writes = "contents: write" in body and "id-token: write" in body
            self.assertEqual(writes, name == "publish", name)
        self.assertIn("needs: [verify, build, extension]", per_job["publish"])

    def test_builds_the_release_targets(self):
        targets = re.findall(r"^\s+target: ([a-z0-9_-]+)$", jobs(self.text)["build"], re.M)
        self.assertEqual(targets, ra.TARGETS)

    def test_signing_tolerates_a_release_without_zip_archives(self):
        """Regression: with Windows left out, `dist/*.zip` matched nothing and reached
        cosign as a literal file name."""
        publish = jobs(self.text)["publish"]
        sign = publish[publish.index("name: Sign") :]
        self.assertLess(sign.index("shopt -s nullglob"), sign.index("dist/*.zip"))

    def test_ci_builds_every_release_target(self):
        """Regression: a target only the release builds fails after the tag is pushed."""
        platforms = jobs(CI.read_text())["platforms"]
        self.assertIn("include: ${{ fromJSON(needs.changes.outputs.platforms) }}", platforms)
        self.assertIn("--release --workspace --bins", platforms)
        self.assertEqual([e["target"] for e in build_targets()], ra.ALL_TARGETS)

    def test_pull_requests_skip_only_intel_mac(self):
        """macOS Intel runners queue for hours, so only that target waits for main."""
        later = [e["target"] for e in build_targets() if e.get("post_merge_only")]
        self.assertEqual(later, ["x86_64-apple-darwin"])

    @unittest.skipUnless(shutil.which("jq"), "jq not installed")
    def test_ci_filter_drops_intel_mac_on_pull_requests_only(self):
        changes = jobs(CI.read_text())["changes"]
        program = re.search(r"jq -c --arg event \"\$EVENT\" '([^']+)' \.github/build-targets\.json", changes)
        self.assertTrue(program, "the changes job does not filter .github/build-targets.json")

        def matrix(event: str) -> list[dict]:
            out = subprocess.run(["jq", "-c", "--arg", "event", event, program[1], str(BUILD_TARGETS)],
                                 capture_output=True, text=True, check=True).stdout
            return json.loads(out)

        on_main = matrix("push")
        self.assertEqual([e["target"] for e in on_main], ra.ALL_TARGETS)
        self.assertTrue(all("post_merge_only" not in e for e in on_main))
        self.assertEqual([e["target"] for e in matrix("pull_request")],
                         [t for t in ra.ALL_TARGETS if t != "x86_64-apple-darwin"])

    def test_the_extension_is_packaged_checked_and_attached(self):
        """`editor-integration` → Extension release assets."""
        extension = jobs(self.text)["extension"]
        self.assertIn("needs: verify", extension)
        self.assertIn("npm run package", extension)
        self.assertIn("release_assets.py vsix-sum", extension)
        self.assertIn("name: archive-vscode-extension", extension)
        publish = jobs(self.text)["publish"]
        self.assertIn("pattern: archive-*", publish)
        self.assertIn("sha256sum -c cyber-$VERSION.vsix.sha256", publish)
        self.assertIn("unsigned", publish)
        verify = jobs(self.text)["verify"]
        self.assertIn("release_assets.py check-tag", verify)

    def test_actions_are_pinned_by_commit(self):
        uses = re.findall(r"uses: (\S+)", self.text)
        self.assertTrue(uses)
        for action in uses:
            self.assertRegex(action, r"^[\w.-]+/[\w.-]+@[0-9a-f]{40}$")

    def test_publishes_to_the_version_and_channel_releases(self):
        publish = jobs(self.text)["publish"]
        self.assertIn('gh release create "v$VERSION" --verify-tag', publish)
        self.assertIn('gh release upload "channel-$CHANNEL" "channel/$CHANNEL.json" --clobber', publish)
        self.assertIn("cosign sign-blob --yes --bundle", publish)
        self.assertIn("release_assets.py assets", publish)

    def test_an_older_tag_never_rolls_the_channel_back(self):
        """Regression: a backport or racing tag must not overwrite `<channel>.json`."""
        self.assertIn("concurrency:\n  group: release\n", self.text)
        publish = jobs(self.text)["publish"]
        order = publish.index("release_assets.py newer --version")
        upload = publish.index('gh release upload "channel-$CHANNEL"')
        self.assertLess(order, publish.index('gh release create "v$VERSION"'))
        self.assertLess(order, upload)
        channel_step = publish[publish.rindex("- name:", 0, upload):upload]
        self.assertIn("if: steps.order.outputs.newer == 'true'", channel_step)
        self.assertIn("kind=--latest=false", publish)

    def test_the_rolling_channel_release_is_never_latest(self):
        """Regression: `channel-beta`, created with `--latest=false` but not a prerelease,
        was GitHub's fallback latest, so `releases/latest/download/install.sh` 404ed."""
        publish = jobs(self.text)["publish"]
        channel_lines = [line.strip() for line in publish.splitlines() if '"channel-$CHANNEL"' in line]
        create = [line for line in channel_lines if line.startswith('gh release create "channel-$CHANNEL"')]
        edit = [line for line in channel_lines if line.startswith('gh release edit "channel-$CHANNEL"')]
        self.assertEqual(len(create), 1)
        self.assertEqual(len(edit), 1, "an existing channel release must be converted too")
        for line in create + edit:
            self.assertIn("--prerelease", line)
            self.assertIn("--latest=false", line)
        for line in channel_lines:
            self.assertNotRegex(line, r"--latest(?!=false)")
        upload = publish.index('gh release upload "channel-$CHANNEL"')
        self.assertLess(publish.index(edit[0]), upload)

    def test_latest_resolves_only_to_a_version_release_with_the_install_script(self):
        """`releases/latest/download/install.sh` (README, `installation-upgrade` → Install
        script): only the `v<version>` release may become latest, and it carries install.sh."""
        publish = jobs(self.text)["publish"]
        self.assertIn("cp scripts/install.sh dist/install.sh", publish)
        self.assertIn("install.sh", ra.release_assets("1.4.0"))
        creates = re.findall(r"^\s*gh release create (\S+)(.*)$", publish, re.M)
        self.assertEqual([tag for tag, _ in creates], ['"v$VERSION"', '"channel-$CHANNEL"'])
        version_create = creates[0][1]
        self.assertIn('"$kind"', version_create)
        self.assertTrue(version_create.rstrip().endswith("dist/*"), "install.sh must ship on the version release")
        self.assertIn("kind=--prerelease", publish)
        self.assertIn('elif [ "$NEWER" = true ]; then kind=--latest\n', publish)
        self.assertNotIn("install.sh", "\n".join(line for line in publish.splitlines() if "channel-$CHANNEL" in line))


if __name__ == "__main__":
    unittest.main()
