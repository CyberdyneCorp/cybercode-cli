#!/usr/bin/env python3
"""Check that every suite task's hidden grader separates broken from fixed workspaces.

For every task in every `kind: "suite"` manifest under eval/manifests:

  1. copy the task fixture to a temporary directory and `git init` it,
  2. run the grader and require FAIL (the untouched fixture must not pass),
  3. overlay eval/solutions/<task-id>/ (files replace or add; paths listed in an
     optional `DELETE` file are removed) and require PASS.

It also checks the grader output contract (final line is exactly PASS or FAIL, exit code
agrees), that the grader leaves the workspace byte-for-byte unchanged, and that the
manifest's `tree_sha256` matches the fixture.

Usage: python3 scripts/check_eval_graders.py [--task ID ...] [--jobs N] [--verbose]
Exit code 1 on any violation.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFESTS = ROOT / "eval" / "manifests"
SOLUTIONS = ROOT / "eval" / "solutions"
SKIPPED_NAMES = {"__pycache__", ".DS_Store"}


def tree_sha256(directory: Path) -> str:
    """Python port of `cyber_core::eval::tree_sha256`."""
    files = sorted(
        (p.relative_to(directory) for p in directory.rglob("*")
         if p.is_file() and not SKIPPED_NAMES.intersection(p.relative_to(directory).parts)),
        key=lambda rel: rel.parts,
    )
    digest = hashlib.sha256()
    for rel in files:
        data = (directory / rel).read_bytes()
        digest.update(rel.as_posix().encode() + b"\0" + str(len(data)).encode() + b"\0" + data)
    return "sha256:" + digest.hexdigest()


def snapshot(directory: Path) -> str:
    """Hash of everything in the workspace, including .git and caches."""
    digest = hashlib.sha256()
    for path in sorted(directory.rglob("*")):
        rel = path.relative_to(directory).as_posix()
        digest.update(rel.encode() + b"\0")
        if path.is_file():
            digest.update(path.read_bytes())
    return digest.hexdigest()


@dataclass
class Task:
    manifest: str
    id: str
    fixture: Path
    tree_sha256: str
    grader_dir: Path
    command: list[str]
    timeout: int


@dataclass
class Outcome:
    task: Task
    violations: list[str] = field(default_factory=list)
    logs: list[str] = field(default_factory=list)


def load_tasks() -> list[Task]:
    tasks = []
    for path in sorted(MANIFESTS.glob("*.json")):
        manifest = json.loads(path.read_text())
        if manifest.get("kind") != "suite":
            continue
        for task in manifest["tasks"]:
            tasks.append(Task(
                manifest=path.name,
                id=task["id"],
                fixture=ROOT / task["fixture"]["path"],
                tree_sha256=task["fixture"]["tree_sha256"],
                grader_dir=ROOT / task["grading"]["grader_dir"],
                command=task["grading"]["command"],
                timeout=task["timeout_seconds"],
            ))
    return tasks


def git(workspace: Path, *args: str) -> None:
    subprocess.run(
        ["git", "-c", "user.name=eval", "-c", "user.email=eval@localhost", *args],
        cwd=workspace, check=True, capture_output=True,
    )


def prepare_workspace(task: Task, workspace: Path) -> None:
    shutil.copytree(task.fixture, workspace, ignore=shutil.ignore_patterns(*SKIPPED_NAMES))
    git(workspace, "init", "-q")
    git(workspace, "add", "-A")
    git(workspace, "commit", "-q", "-m", "fixture")


def overlay_solution(solution: Path, workspace: Path) -> None:
    deletions = solution / "DELETE"
    for src in solution.rglob("*"):
        if src.is_file() and src != deletions and not SKIPPED_NAMES.intersection(src.parts):
            dst = workspace / src.relative_to(solution)
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dst)
    if deletions.exists():
        for rel in filter(None, map(str.strip, deletions.read_text().splitlines())):
            target = workspace / rel
            if target.is_dir():
                shutil.rmtree(target)
            else:
                target.unlink(missing_ok=True)


def grade(task: Task, workspace: Path, expect_pass: bool, outcome: Outcome) -> None:
    stage = "solution" if expect_pass else "fixture"
    command = [
        part.replace("{grader_dir}", str(task.grader_dir)).replace("{workspace}", str(workspace))
        for part in task.command
    ]
    before = snapshot(workspace)
    try:
        result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=task.timeout)
    except subprocess.TimeoutExpired:
        outcome.violations.append(f"{stage}: grader timed out after {task.timeout}s")
        return
    output = (result.stdout + result.stderr).strip()
    outcome.logs.append(f"--- {stage} (exit {result.returncode})\n{output}")
    lines = result.stdout.strip().splitlines()
    verdict = lines[-1] if lines else ""
    if verdict not in ("PASS", "FAIL"):
        outcome.violations.append(f"{stage}: last stdout line is {verdict!r}, expected PASS or FAIL")
    elif (verdict == "PASS") != (result.returncode == 0):
        outcome.violations.append(f"{stage}: verdict {verdict} disagrees with exit {result.returncode}")
    passed = result.returncode == 0
    if passed != expect_pass:
        outcome.violations.append(f"{stage}: grader {'passed' if passed else 'failed'}, expected "
                                  f"{'PASS' if expect_pass else 'FAIL'}")
    if snapshot(workspace) != before:
        outcome.violations.append(f"{stage}: grader modified the workspace")


def check(task: Task) -> Outcome:
    outcome = Outcome(task)
    solution = SOLUTIONS / task.id
    if not task.fixture.is_dir():
        outcome.violations.append(f"fixture {task.fixture} is missing")
        return outcome
    if not solution.is_dir():
        outcome.violations.append(f"solution {solution} is missing")
        return outcome
    actual = tree_sha256(task.fixture)
    if actual != task.tree_sha256:
        outcome.violations.append(f"tree_sha256 is {actual}, manifest says {task.tree_sha256}")
    with tempfile.TemporaryDirectory(prefix=f"eval-{task.id}-") as tmp:
        workspace = Path(tmp) / "workspace"
        prepare_workspace(task, workspace)
        grade(task, workspace, expect_pass=False, outcome=outcome)
        overlay_solution(solution, workspace)
        grade(task, workspace, expect_pass=True, outcome=outcome)
    return outcome


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--task", action="append", help="only check these task ids")
    parser.add_argument("--jobs", type=int, default=4, help="tasks checked in parallel")
    parser.add_argument("--verbose", action="store_true", help="print grader output for every task")
    args = parser.parse_args()

    tasks = load_tasks()
    if args.task:
        tasks = [t for t in tasks if t.id in set(args.task)]
    if not tasks:
        print("no suite tasks found")
        return 1
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        outcomes = list(pool.map(check, tasks))

    failed = 0
    for outcome in outcomes:
        status = "ok  " if not outcome.violations else "FAIL"
        print(f"{status} {outcome.task.manifest}:{outcome.task.id}")
        for violation in outcome.violations:
            print(f"     - {violation}")
        if outcome.violations or args.verbose:
            for log in outcome.logs:
                print("     " + log.replace("\n", "\n     "))
        failed += bool(outcome.violations)
    print(f"\n{len(outcomes) - failed}/{len(outcomes)} tasks: grader fails the fixture and passes the solution")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
