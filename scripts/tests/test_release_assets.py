"""scripts/release_assets.py (`installation-upgrade` → Release targets "Release assets",
Package channels "Rendered manifests")."""

import datetime as dt
import hashlib
import json
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import release_assets as ra  # noqa: E402

VERSION = "1.4.0"


def fake_release(dist: Path) -> None:
    """Archives for every target, each with distinct bytes."""
    dist.mkdir(parents=True, exist_ok=True)
    for target in ra.TARGETS:
        (dist / ra.archive_name(VERSION, target)).write_bytes(target.encode())


class ChannelAndTag(unittest.TestCase):
    def test_channel_from_the_version(self):
        self.assertEqual(ra.channel_of("1.4.0"), "stable")
        self.assertEqual(ra.channel_of("1.5.0-beta.2"), "beta")
        self.assertEqual(ra.channel_of("1.5.0-nightly.20261023"), "nightly")
        for bad in ["1.5.0-rc.1", "1.5.0-nightly.1", "1.5", "v1.4.0"]:
            with self.assertRaises(ra.ReleaseError, msg=bad):
                ra.channel_of(bad)

    def test_tag_must_match_the_workspace_version(self):
        with tempfile.TemporaryDirectory() as tmp:
            cargo = Path(tmp) / "Cargo.toml"
            cargo.write_text('[workspace.package]\nversion = "1.5.0-beta.1"\n')
            self.assertEqual(ra.check_tag("v1.5.0-beta.1", cargo), ("1.5.0-beta.1", "beta"))
            with self.assertRaises(ra.ReleaseError):
                ra.check_tag("v1.5.0", cargo)

    def test_repository_version_is_releasable(self):
        version = ra.workspace_version(ra.ROOT / "Cargo.toml")
        self.assertIn(ra.channel_of(version), {"stable", "beta", "nightly"})


def packaged_release(tmp: Path) -> Path:
    """Real archives for every target, packaged by `release_assets.package`, whose
    binaries hold `cyber <target>` and `helper <target>`."""
    dist = tmp / "dist"
    for target in ra.TARGETS:
        ext = ".exe" if ra.is_windows(target) else ""
        bin_dir = tmp / "bin" / target
        bin_dir.mkdir(parents=True)
        (bin_dir / f"cyber{ext}").write_text(f"cyber {target}")
        (bin_dir / f"cyber-sandbox-exec{ext}").write_text(f"helper {target}")
        ra.package(VERSION, target, bin_dir, dist)
    return dist


class ReleaseAssets(unittest.TestCase):
    def test_eight_targets_with_windows_zips(self):
        self.assertEqual(len(ra.TARGETS), 8)
        names = [ra.archive_name(VERSION, t) for t in ra.TARGETS]
        self.assertIn("cyber-1.4.0-x86_64-unknown-linux-musl.tar.gz", names)
        self.assertIn("cyber-1.4.0-aarch64-pc-windows-msvc.zip", names)
        self.assertEqual(sum(n.endswith(".zip") for n in names), 2)

    def test_release_asset_names(self):
        assets = ra.release_assets(VERSION)
        archives = [a for a in assets if a.endswith((".tar.gz", ".zip")) and "npm" not in a]
        self.assertEqual(len(archives), 8)
        for archive in archives:
            self.assertIn(f"{archive}.sigstore.json", assets)
        for name in ["SHA256SUMS", "SHA256SUMS.sigstore.json", "install.sh"]:
            self.assertIn(name, assets)
        for name in ["cyber.rb", "cyber.scoop.json", "cyber.winget.yaml", "PKGBUILD"]:
            self.assertIn(name, assets)

    def test_sums_cover_every_archive_and_refuse_a_missing_one(self):
        with tempfile.TemporaryDirectory() as tmp:
            dist = Path(tmp)
            fake_release(dist)
            sums = ra.parse_sums(ra.write_sums(VERSION, dist))
            self.assertEqual(len(sums), 8)
            name = ra.archive_name(VERSION, "aarch64-apple-darwin")
            self.assertEqual(sums[name], hashlib.sha256(b"aarch64-apple-darwin").hexdigest())
            # The strict form `cyber upgrade` parses: `<64 hex>  <name>\n` per line.
            text = (dist / "SHA256SUMS").read_text()
            self.assertTrue(text.endswith("\n") and "\r" not in text)
            (dist / name).unlink()
            with self.assertRaises(ra.ReleaseError):
                ra.write_sums(VERSION, dist)

    def test_check_assets_wants_exactly_the_release(self):
        with tempfile.TemporaryDirectory() as tmp:
            dist = Path(tmp)
            for name in ra.release_assets(VERSION):
                (dist / name).write_text("x")
            ra.check_assets(VERSION, dist)
            (dist / "stray.txt").write_text("x")
            with self.assertRaises(ra.ReleaseError):
                ra.check_assets(VERSION, dist)
            (dist / "stray.txt").unlink()
            (dist / "install.sh").unlink()
            with self.assertRaises(ra.ReleaseError):
                ra.check_assets(VERSION, dist)

    def test_channel_manifest(self):
        now = dt.datetime(2026, 10, 23, 12, 0, tzinfo=dt.timezone.utc)
        manifest = ra.channel_manifest(VERSION, "https://example.test/v1.4.0", "1.2.0", now)
        self.assertEqual(
            manifest,
            {
                "version": "1.4.0",
                "published_at": "2026-10-23T12:00:00Z",
                "notes_url": "https://example.test/v1.4.0",
                "min_supported": "1.2.0",
            },
        )
        self.assertNotIn("min_supported", ra.channel_manifest(VERSION, "u", "", now))
        with self.assertRaises(ra.ReleaseError):
            ra.channel_manifest(VERSION, "u", "latest", now)


class ChannelOrder(unittest.TestCase):
    """Regression: a backport or racing tag never rolls the channel manifest back."""

    def test_semver_precedence(self):
        ordered = ["1.3.5", "1.4.0-beta.2", "1.4.0-beta.10", "1.4.0", "1.4.1", "1.10.0"]
        self.assertEqual(sorted(ordered, key=ra.semver_key), ordered)
        self.assertLess(ra.semver_key("1.5.0-nightly.20261022"), ra.semver_key("1.5.0-nightly.20261023"))
        with self.assertRaises(ra.ReleaseError):
            ra.semver_key("latest")

    def test_only_a_newer_version_replaces_the_manifest(self):
        with tempfile.TemporaryDirectory() as tmp:
            current = Path(tmp) / "stable.json"
            self.assertTrue(ra.is_newer("1.3.5", current), "a first release")
            current.write_text(json.dumps({"version": "1.4.0"}))
            self.assertFalse(ra.is_newer("1.3.5", current))
            self.assertFalse(ra.is_newer("1.4.0", current))
            self.assertTrue(ra.is_newer("1.4.1", current))
            current.write_text(json.dumps({"version": "garbage"}))
            with self.assertRaises(ra.ReleaseError):
                ra.is_newer("1.4.1", current)


class Packaging(unittest.TestCase):
    def binaries(self, tmp: Path, ext: str) -> Path:
        bin_dir = tmp / "bin"
        bin_dir.mkdir()
        for name in ("cyber", "cyber-sandbox-exec"):
            (bin_dir / f"{name}{ext}").write_bytes(name.encode())
        return bin_dir

    def test_unix_archive_has_executables_at_the_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            target = "x86_64-unknown-linux-musl"
            path = ra.package(VERSION, target, self.binaries(tmp, ""), tmp / "dist")
            self.assertEqual(path.name, ra.archive_name(VERSION, target))
            with tarfile.open(path) as tar:
                members = {m.name: m for m in tar.getmembers()}
            self.assertEqual(members["cyber"].mode, 0o755)
            self.assertEqual(members["cyber-sandbox-exec"].mode, 0o755)
            self.assertIn("README.md", members)
            self.assertNotIn("PREVIEW.txt", members)
            self.assertTrue(all("/" not in name for name in members))
            # Reproducible: the same inputs give the same bytes.
            again = ra.package(VERSION, target, tmp / "bin", tmp / "again")
            self.assertEqual(path.read_bytes(), again.read_bytes())

    def test_windows_archive_is_a_preview_zip(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            target = "aarch64-pc-windows-msvc"
            path = ra.package(VERSION, target, self.binaries(tmp, ".exe"), tmp / "dist")
            with zipfile.ZipFile(path) as archive:
                names = archive.namelist()
            self.assertIn("cyber.exe", names)
            self.assertIn("cyber-sandbox-exec.exe", names)
            self.assertIn("PREVIEW.txt", names)

    def test_missing_binary_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            (tmp / "bin").mkdir()
            with self.assertRaises(ra.ReleaseError):
                ra.package(VERSION, "aarch64-apple-darwin", tmp / "bin", tmp / "dist")


class RenderedManifests(unittest.TestCase):
    def render(self, tmp: Path) -> tuple[dict[str, str], Path]:
        dist = packaged_release(tmp)
        sums = ra.parse_sums(ra.write_sums(VERSION, dist))
        out = tmp / "rendered"
        ra.render_packaging(
            VERSION, (dist / "SHA256SUMS").read_text(), ra.ROOT / "packaging", out,
            ra.DEFAULT_BASE_URL, dist,
        )
        return sums, out

    def test_manifests_name_the_version_and_checksums(self):
        with tempfile.TemporaryDirectory() as tmp:
            sums, out = self.render(Path(tmp))
            expected = {
                "cyber.rb": ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu"],
                "cyber.scoop.json": ["x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"],
                "cyber.winget.yaml": ["x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"],
                "PKGBUILD": ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"],
                "cyber.nix": ["aarch64-apple-darwin", "x86_64-unknown-linux-musl"],
            }
            for asset, targets in expected.items():
                text = (out / asset).read_text()
                self.assertNotIn("{{", text, asset)
                self.assertIn(VERSION, text, asset)
                for target in targets:
                    name = ra.archive_name(VERSION, target)
                    self.assertIn(sums[name], text, f"{asset}: {target}")
                    self.assertIn(f"{ra.DEFAULT_BASE_URL}/v{VERSION}/{name}", text)
            json.loads((out / "cyber.scoop.json").read_text())

    def test_npm_packages(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, out = self.render(Path(tmp))
            with tarfile.open(out / f"cyber-npm-{VERSION}.tar.gz") as tar:
                members = {m.name: m for m in tar.getmembers()}
                files = {name: tar.extractfile(m).read() for name, m in members.items()}
            cli = json.loads(files["cli/package.json"])
            self.assertEqual(cli["version"], VERSION)
            self.assertEqual(cli["bin"], {"cyber": "bin/cyber.js"})
            self.assertIn("cli/bin/cyber.js", files)
            for key in ra.NPM_PLATFORMS:
                name = f"@cyber-code/cli-{key}"
                self.assertEqual(cli["optionalDependencies"][name], VERSION)
                platform = json.loads(files[f"cli-{key}/package.json"])
                os_name, cpu = key.split("-")
                self.assertEqual(
                    (platform["name"], platform["os"], platform["cpu"]),
                    (name, [os_name], [cpu]),
                )
                # Regression: each platform package holds its mapped target's binaries
                # (Linux: the static musl builds), executable, where the launcher runs it.
                target = ra.NPM_PLATFORMS[key]
                ext = ".exe" if os_name == "win32" else ""
                for binary, content in (("cyber", "cyber"), ("cyber-sandbox-exec", "helper")):
                    path = f"cli-{key}/bin/{binary}{ext}"
                    self.assertEqual(files[path], f"{content} {target}".encode(), path)
                    self.assertEqual(members[path].mode, 0o755, path)
            self.assertEqual(members["cli/bin/cyber.js"].mode, 0o755)

    def test_npm_packages_refuse_an_archive_that_does_not_match_the_sums(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            dist = packaged_release(tmp)
            sums_text = ra.write_sums(VERSION, dist)
            (dist / ra.archive_name(VERSION, "x86_64-unknown-linux-musl")).write_bytes(b"x")
            with self.assertRaises(ra.ReleaseError):
                ra.render_packaging(
                    VERSION, sums_text, ra.ROOT / "packaging", tmp / "out",
                    ra.DEFAULT_BASE_URL, dist,
                )

    def test_unknown_placeholder_is_an_error(self):
        with self.assertRaises(ra.ReleaseError):
            ra.render("{{ version }} {{ sha256.sparc-sun-solaris }}", {"version": "1"})

    def test_sums_without_a_target_are_refused(self):
        with self.assertRaises(ra.ReleaseError):
            ra.variables(VERSION, {}, ra.DEFAULT_BASE_URL)


if __name__ == "__main__":
    unittest.main()
