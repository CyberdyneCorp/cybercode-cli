"""Find the files under a root that belong in the index.

A file is indexed when it is a regular file (symlinks are neither followed nor indexed)
whose name ends in one of INDEXED_SUFFIXES, no component of its relative path starts with
"." and no ignore rule matches it. Ignore rules are glob patterns (fnmatch, case-sensitive):
a rule containing "/" is matched against the whole relative POSIX path (e.g. "docs/drafts/*"),
any other rule against each path component (e.g. "build" or "*.tmp.txt"). A directory that
matches a rule is skipped with everything below it.
"""

from __future__ import annotations

import os
from fnmatch import fnmatchcase
from pathlib import Path

from .config import IndexConfig

INDEXED_SUFFIXES = (".txt", ".md")
IGNORE_FILE = ".indexignore"


def load_ignore_rules(root: str | os.PathLike, config: IndexConfig) -> tuple[str, ...]:
    """The effective rules: `config.ignore`, then the lines of `<root>/.indexignore`.

    Lines are stripped; blank lines and lines starting with "#" are skipped.
    """
    rules = list(config.ignore)
    path = Path(root) / IGNORE_FILE
    if path.is_file():
        for line in path.read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if line and not line.startswith("#"):
                rules.append(line)
    return tuple(rules)


def is_ignored(relpath: str, rules: tuple[str, ...]) -> bool:
    name = relpath.rsplit("/", 1)[-1]
    if name.startswith("."):
        return True
    for rule in rules:
        if "/" in rule:
            if fnmatchcase(relpath, rule):
                return True
        elif fnmatchcase(name, rule):
            return True
    return False


def walk(root: str | os.PathLike, rules: tuple[str, ...]) -> list[str]:
    """Relative POSIX paths of the indexable files, sorted by plain string order."""
    root = os.fspath(root)
    found = []
    for dirpath, dirnames, filenames in os.walk(root):
        reldir = os.path.relpath(dirpath, root).replace(os.sep, "/")
        prefix = "" if reldir == "." else reldir + "/"
        dirnames[:] = [d for d in dirnames if not is_ignored(prefix + d, rules)]
        for filename in filenames:
            relpath = prefix + filename
            full = os.path.join(dirpath, filename)
            if not filename.endswith(INDEXED_SUFFIXES) or is_ignored(relpath, rules):
                continue
            if os.path.islink(full) or not os.path.isfile(full):
                continue
            found.append(relpath)
    return sorted(found)
