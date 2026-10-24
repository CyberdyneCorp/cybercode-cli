"""`.github/workflows/release.yml` (`installation-upgrade` → Release targets "Not on pull
requests"), checked as text: YAML 1.1 parsers read the `on` key as a boolean."""

import re
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import release_assets as ra  # noqa: E402

WORKFLOW = ra.ROOT / ".github" / "workflows" / "release.yml"


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
        self.assertEqual(set(per_job), {"verify", "build", "publish"})
        for name, body in per_job.items():
            writes = "contents: write" in body and "id-token: write" in body
            self.assertEqual(writes, name == "publish", name)
        self.assertIn("needs: [verify, build]", per_job["publish"])

    def test_builds_the_eight_release_targets(self):
        targets = re.findall(r"^\s+target: ([a-z0-9_-]+)$", jobs(self.text)["build"], re.M)
        self.assertEqual(targets, ra.TARGETS)

    def test_ci_builds_every_release_target_on_pull_requests(self):
        """Regression: a target only the release builds fails after the tag is pushed."""
        ci = (ra.ROOT / ".github" / "workflows" / "ci.yml").read_text()
        platforms = jobs(ci)["platforms"]
        targets = re.findall(r"^\s+target: ([a-z0-9_-]+)$", platforms, re.M)
        self.assertEqual(targets, ra.TARGETS)
        self.assertIn("--release --workspace --bins", platforms)

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


if __name__ == "__main__":
    unittest.main()
