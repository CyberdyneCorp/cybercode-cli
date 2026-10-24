#!/usr/bin/env python3
"""Release assets for `.github/workflows/release.yml` (`installation-upgrade` → Release
targets, Package channels).

Subcommands:
  check-tag TAG            the tag must be `v` + the workspace version; prints
                           `version=<v>` and `channel=<c>` for $GITHUB_OUTPUT
  package                  one target's archive, binaries at the archive root
  sums DIR                 `SHA256SUMS` over the 8 archives in DIR (all must be there)
  manifest                 the channel manifest `<channel>.json`
  newer                    whether the version is newer by semver than the published
                           channel manifest (`--current`); prints `newer=true|false`
  render                   the `packaging/` templates, with the version and checksums,
                           and the npm packages with the binaries of the archives
  assets DIR               check DIR holds exactly the release's assets

Only the standard library is used, so every runner (Windows included) can run it.
"""

from __future__ import annotations

import argparse
import datetime as dt
import gzip
import hashlib
import io
import json
import re
import sys
import tarfile
import tomllib
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_BASE_URL = "https://github.com/CyberdyneCorp/cybercode-cli/releases/download"

# The release targets, in `ci.yml` order, then Windows.
TARGETS = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
]

# npm platform packages (`@cyber-code/cli-<key>`): `process.platform`/`process.arch` to
# the target. Linux uses the static musl builds, so one package serves every libc.
NPM_PLATFORMS = {
    "darwin-arm64": "aarch64-apple-darwin",
    "darwin-x64": "x86_64-apple-darwin",
    "linux-arm64": "aarch64-unknown-linux-musl",
    "linux-x64": "x86_64-unknown-linux-musl",
    "win32-arm64": "aarch64-pc-windows-msvc",
    "win32-x64": "x86_64-pc-windows-msvc",
}

# Template (under packaging/) to the release asset it renders to.
TEMPLATES = {
    "homebrew/cyber.rb": "cyber.rb",
    "scoop/cyber.json": "cyber.scoop.json",
    "winget/cyber-code.cyber.yaml": "cyber.winget.yaml",
    "aur/PKGBUILD": "PKGBUILD",
    "nix/default.nix": "cyber.nix",
}

VERSION_RE = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$")
PLACEHOLDER = re.compile(r"\{\{ *([a-z0-9_.-]+) *\}\}")
PREVIEW = (
    "Windows builds of Cyber Code are a preview: sandbox enforcement on Windows has not\n"
    "passed its release gate yet.\n"
)


class ReleaseError(Exception):
    pass


def is_windows(target: str) -> bool:
    return "windows" in target


def archive_name(version: str, target: str) -> str:
    ext = "zip" if is_windows(target) else "tar.gz"
    return f"cyber-{version}-{target}.{ext}"


def channel_of(version: str) -> str:
    """`-beta.N` is beta, `-nightly.YYYYMMDD` is nightly, no prerelease is stable."""
    if not VERSION_RE.match(version):
        raise ReleaseError(f"version {version!r} is not semver")
    _, _, pre = version.partition("-")
    if not pre:
        return "stable"
    if re.fullmatch(r"beta\.[0-9]+", pre):
        return "beta"
    if re.fullmatch(r"nightly\.[0-9]{8}", pre):
        return "nightly"
    raise ReleaseError(f"prerelease {pre!r} is neither beta.N nor nightly.YYYYMMDD")


def workspace_version(cargo_toml: Path) -> str:
    data = tomllib.loads(cargo_toml.read_text())
    return data["workspace"]["package"]["version"]


def check_tag(tag: str, cargo_toml: Path) -> tuple[str, str]:
    version = workspace_version(cargo_toml)
    if tag != f"v{version}":
        raise ReleaseError(f"tag {tag} does not match the workspace version {version}")
    return version, channel_of(version)


def release_assets(version: str) -> list[str]:
    """Every asset of release `v<version>`, sorted."""
    archives = [archive_name(version, t) for t in TARGETS]
    names = archives + [f"{a}.sigstore.json" for a in archives]
    names += ["SHA256SUMS", "SHA256SUMS.sigstore.json", "install.sh"]
    names += list(TEMPLATES.values()) + [f"cyber-npm-{version}.tar.gz"]
    return sorted(names)


# --- packaging an archive -------------------------------------------------------------


def archive_members(target: str, bin_dir: Path) -> list[tuple[str, bytes, int]]:
    """(name, bytes, mode) of each member, all at the archive root."""
    ext = ".exe" if is_windows(target) else ""
    members = []
    for name in ("cyber", "cyber-sandbox-exec"):
        path = bin_dir / f"{name}{ext}"
        if not path.is_file():
            raise ReleaseError(f"missing binary {path}")
        members.append((f"{name}{ext}", path.read_bytes(), 0o755))
    for doc in ("README.md", "LICENSE"):
        if (ROOT / doc).is_file():
            members.append((doc, (ROOT / doc).read_bytes(), 0o644))
    if is_windows(target):
        members.append(("PREVIEW.txt", PREVIEW.encode(), 0o644))
    return members


def write_tar_gz(path: Path, members: list[tuple[str, bytes, int]]) -> None:
    # mtime 0 and fixed owners: the same inputs give the same bytes.
    with path.open("wb") as out:
        with gzip.GzipFile(fileobj=out, mode="wb", mtime=0) as gz:
            with tarfile.open(fileobj=gz, mode="w", format=tarfile.PAX_FORMAT) as tar:
                for name, data, mode in members:
                    info = tarfile.TarInfo(name)
                    info.size, info.mode, info.mtime = len(data), mode, 0
                    tar.addfile(info, io.BytesIO(data))


def write_zip(path: Path, members: list[tuple[str, bytes, int]]) -> None:
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, data, mode in members:
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.external_attr = (0o100000 | mode) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, data)


def package(version: str, target: str, bin_dir: Path, out: Path) -> Path:
    if target not in TARGETS:
        raise ReleaseError(f"unknown target {target}")
    members = archive_members(target, bin_dir)
    out.mkdir(parents=True, exist_ok=True)
    path = out / archive_name(version, target)
    (write_zip if is_windows(target) else write_tar_gz)(path, members)
    return path


# --- checksums, manifest and templates ------------------------------------------------


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_sums(version: str, dist: Path) -> str:
    """`SHA256SUMS` in the strict form `cyber upgrade` parses; every archive must exist."""
    lines = []
    for name in sorted(archive_name(version, t) for t in TARGETS):
        path = dist / name
        if not path.is_file():
            raise ReleaseError(f"missing archive {name}")
        lines.append(f"{sha256(path)}  {name}\n")
    text = "".join(lines)
    (dist / "SHA256SUMS").write_text(text, newline="\n")
    return text


def parse_sums(text: str) -> dict[str, str]:
    sums = {}
    for line in text.splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (\S+)", line)
        if not match or match[2] in sums:
            raise ReleaseError(f"bad SHA256SUMS line {line!r}")
        sums[match[2]] = match[1]
    return sums


def channel_manifest(
    version: str, notes_url: str, min_supported: str | None, now: dt.datetime
) -> dict:
    manifest = {
        "version": version,
        "published_at": now.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "notes_url": notes_url,
    }
    if min_supported:
        if not VERSION_RE.match(min_supported):
            raise ReleaseError(f"min_supported {min_supported!r} is not semver")
        manifest["min_supported"] = min_supported
    return manifest


def semver_key(version: str) -> tuple:
    """A sort key in semver precedence: a pre-release sorts before its release, numeric
    identifiers numerically and before alphanumeric ones."""
    if not VERSION_RE.match(version):
        raise ReleaseError(f"version {version!r} is not semver")
    core, _, pre = version.partition("-")
    numbers = tuple(int(part) for part in core.split("."))
    if not pre:
        return numbers, (1,)
    ids = tuple((0, int(i), "") if i.isdigit() else (1, 0, i) for i in pre.split("."))
    return numbers, (0, ids)


def is_newer(version: str, current: Path) -> bool:
    """Whether `version` may replace the channel manifest at `current` (missing: the
    channel's first release). An older or equal tag (a backport, a racing tag) must
    not roll the channel back."""
    if not current.is_file():
        return True
    published = json.loads(current.read_text()).get("version", "")
    return semver_key(version) > semver_key(str(published))


def variables(version: str, sums: dict[str, str], base_url: str) -> dict[str, str]:
    values = {"version": version, "base_url": base_url}
    for target in TARGETS:
        name = archive_name(version, target)
        if name not in sums:
            raise ReleaseError(f"SHA256SUMS has no line for {name}")
        values[f"sha256.{target}"] = sums[name]
        values[f"url.{target}"] = f"{base_url}/v{version}/{name}"
    return values


def render(text: str, values: dict[str, str]) -> str:
    """Replace `{{ key }}`; an unknown key is an error, never left in the output."""

    def substitute(match: re.Match) -> str:
        key = match[1]
        if key not in values:
            raise ReleaseError(f"unknown template key {key!r}")
        return values[key]

    return PLACEHOLDER.sub(substitute, text)


def archive_binaries(path: Path, target: str, expected_sha256: str) -> dict[str, bytes]:
    """`cyber` and `cyber-sandbox-exec` (`.exe` on Windows) from a release archive whose
    SHA-256 must be the one `SHA256SUMS` names, by file name."""
    if not path.is_file():
        raise ReleaseError(f"missing archive {path.name}")
    if sha256(path) != expected_sha256:
        raise ReleaseError(f"{path.name} does not match SHA256SUMS")
    ext = ".exe" if is_windows(target) else ""
    names = [f"{name}{ext}" for name in ("cyber", "cyber-sandbox-exec")]
    try:
        if is_windows(target):
            with zipfile.ZipFile(path) as archive:
                return {name: archive.read(name) for name in names}
        with tarfile.open(path) as archive:
            members = {name: archive.getmember(name) for name in names}
            if not all(member.isfile() for member in members.values()):
                raise ReleaseError(f"{path.name}: a binary is not a regular file")
            return {name: archive.extractfile(m).read() for name, m in members.items()}
    except (KeyError, tarfile.TarError, zipfile.BadZipFile) as error:
        raise ReleaseError(f"{path.name} does not hold {names}: {error}") from error


def npm_files(
    templates: Path, values: dict[str, str], dist: Path, sums: dict[str, str]
) -> list[tuple[str, bytes, int]]:
    """The `@cyber-code/cli` package and one package per platform, each holding the
    binaries of its target's archive under `bin/`, as (path, bytes, mode)."""
    files = []
    cli = templates / "npm" / "cli"
    for path in sorted(p for p in cli.rglob("*") if p.is_file()):
        rel = path.relative_to(cli).as_posix()
        mode = 0o755 if rel.startswith("bin/") else 0o644
        files.append((f"cli/{rel}", render(path.read_text(), values).encode(), mode))
    platform = (templates / "npm" / "platform" / "package.json").read_text()
    version = values["version"]
    for key, target in NPM_PLATFORMS.items():
        os_name, cpu = key.split("-")
        extra = {"npm.key": key, "npm.os": os_name, "npm.cpu": cpu}
        package_json = render(platform, values | extra).encode()
        files.append((f"cli-{key}/package.json", package_json, 0o644))
        name = archive_name(version, target)
        for binary, data in archive_binaries(dist / name, target, sums[name]).items():
            files.append((f"cli-{key}/bin/{binary}", data, 0o755))
    return files


def render_packaging(
    version: str, sums_text: str, templates: Path, out: Path, base_url: str, dist: Path
) -> list[Path]:
    sums = parse_sums(sums_text)
    values = variables(version, sums, base_url)
    out.mkdir(parents=True, exist_ok=True)
    written = []
    for template, asset in TEMPLATES.items():
        path = out / asset
        path.write_text(render((templates / template).read_text(), values), newline="\n")
        written.append(path)
    npm = out / f"cyber-npm-{version}.tar.gz"
    write_tar_gz(npm, npm_files(templates, values, dist, sums))
    return written + [npm]


def check_assets(version: str, dist: Path) -> None:
    present = sorted(p.name for p in dist.iterdir() if p.is_file())
    expected = release_assets(version)
    missing = sorted(set(expected) - set(present))
    extra = sorted(set(present) - set(expected))
    if missing or extra:
        raise ReleaseError(f"release assets differ: missing {missing}, unexpected {extra}")


# --- command line ---------------------------------------------------------------------


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    tag = sub.add_parser("check-tag")
    tag.add_argument("tag")
    tag.add_argument("--cargo-toml", type=Path, default=ROOT / "Cargo.toml")
    pkg = sub.add_parser("package")
    pkg.add_argument("--version", required=True)
    pkg.add_argument("--target", required=True)
    pkg.add_argument("--bin-dir", type=Path, required=True)
    pkg.add_argument("--out", type=Path, required=True)
    sums = sub.add_parser("sums")
    sums.add_argument("--version", required=True)
    sums.add_argument("dist", type=Path)
    manifest = sub.add_parser("manifest")
    manifest.add_argument("--version", required=True)
    manifest.add_argument("--notes-url", required=True)
    manifest.add_argument("--min-supported", default="")
    manifest.add_argument("--out", type=Path, required=True)
    newer = sub.add_parser("newer")
    newer.add_argument("--version", required=True)
    newer.add_argument("--current", type=Path, required=True)
    rend = sub.add_parser("render")
    rend.add_argument("--version", required=True)
    rend.add_argument("--sums", type=Path, required=True)
    rend.add_argument("--templates", type=Path, default=ROOT / "packaging")
    rend.add_argument("--base-url", default=DEFAULT_BASE_URL)
    rend.add_argument("--dist", type=Path, help="the archives; defaults to --sums' directory")
    rend.add_argument("--out", type=Path, required=True)
    assets = sub.add_parser("assets")
    assets.add_argument("--version", required=True)
    assets.add_argument("dist", type=Path)
    return parser.parse_args(argv)


def run(args: argparse.Namespace) -> None:
    if args.command == "check-tag":
        version, channel = check_tag(args.tag, args.cargo_toml)
        print(f"version={version}\nchannel={channel}")
    elif args.command == "package":
        print(package(args.version, args.target, args.bin_dir, args.out))
    elif args.command == "sums":
        sys.stdout.write(write_sums(args.version, args.dist))
    elif args.command == "manifest":
        now = dt.datetime.now(dt.timezone.utc)
        body = channel_manifest(args.version, args.notes_url, args.min_supported, now)
        args.out.write_text(json.dumps(body, indent=2) + "\n", newline="\n")
    elif args.command == "newer":
        print(f"newer={str(is_newer(args.version, args.current)).lower()}")
    elif args.command == "render":
        dist = args.dist or args.sums.parent
        for path in render_packaging(
            args.version, args.sums.read_text(), args.templates, args.out, args.base_url, dist
        ):
            print(path)
    else:
        check_assets(args.version, args.dist)


def main(argv: list[str]) -> int:
    try:
        run(parse_args(argv))
    except ReleaseError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
