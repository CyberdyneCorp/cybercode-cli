#!/usr/bin/env python3
"""Paired Cyber vs Claude Code pilot on the coding suites, same Anthropic model.

Modes (exactly one):
  --dry-run       validate inputs, graders, binaries, flags and isolated config; no model call
  --estimate      print the cost estimate and its arithmetic; no model call
  --smoke         one unscored task per harness (paid; needs --confirm-spend)
  --run           the scored 40-task run per harness (paid; needs --confirm-spend)
  --verify        check a finished run against its recorded inputs and binaries

The Anthropic key is read only from ANTHROPIC_API_KEY or from the file named by
CYBER_EVAL_ENV_FILE (an `ANTHROPIC_API_KEY=...` line). It is never printed or logged, is
passed only to the two candidate processes, never to graders, and is scrubbed from every
file a trial writes. See docs/measurements/claude-code-pilot-plan.md.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
import platform
import re
import shutil
import signal
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CONFIG = ROOT / "eval" / "pilots" / "claude-code-pilot.json"
KEY_VAR = "ANTHROPIC_API_KEY"
ENV_FILE_VAR = "CYBER_EVAL_ENV_FILE"
HARNESSES = ("cyber", "claude-code")
REDACTED = "[REDACTED]"
TOKEN_CLASSES = ("input", "output", "cache_read", "cache_write")

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_eval_graders as graders  # noqa: E402


class PilotError(Exception):
    """A refusal or setup failure reported without a traceback."""


# ---------------------------------------------------------------- configuration


@dataclass
class Task:
    suite: str
    manifest: Path
    id: str
    prompt: str
    fixture: Path
    tree_sha256: str
    grader_dir: Path
    grade_command: list[str]
    timeout: int
    budget_usd: float


@dataclass
class Check:
    name: str
    ok: bool
    detail: str = ""
    warning: bool = False

    def line(self) -> str:
        status = "ok" if self.ok else ("warn" if self.warning else "FAIL")
        return f"[{status:4}] {self.name}" + (f": {self.detail}" if self.detail else "")


def load_config(path: Path) -> dict:
    config = json.loads(path.read_text())
    if config.get("schema_version") != 1:
        raise PilotError(f"{path}: unsupported schema_version")
    return config


def load_tasks(config: dict) -> list[Task]:
    tasks = []
    for rel in config["suites"]:
        manifest = ROOT / rel
        data = json.loads(manifest.read_text())
        for t in data["tasks"]:
            tasks.append(
                Task(
                    suite=data["id"],
                    manifest=manifest,
                    id=t["id"],
                    prompt=t["prompt"],
                    fixture=ROOT / t["fixture"]["path"],
                    tree_sha256=t["fixture"]["tree_sha256"],
                    grader_dir=ROOT / t["grading"]["grader_dir"],
                    grade_command=t["grading"]["command"],
                    timeout=min(t["timeout_seconds"], t["budget"].get("max_wall_seconds", t["timeout_seconds"])),
                    budget_usd=float(t["budget"]["max_cost_usd"]),
                )
            )
    return tasks


def frozen_plan(tasks: list[Task]) -> list[dict]:
    """Manifest order; the harness that goes first alternates per task."""
    plan = []
    for index, task in enumerate(tasks):
        order = list(HARNESSES) if index % 2 == 0 else list(reversed(HARNESSES))
        plan.append({"suite": task.suite, "task": task.id, "order": order})
    return plan


# ---------------------------------------------------------------- secrets


def load_api_key(environ: dict[str, str]) -> tuple[str | None, str]:
    """The key and where it came from ("env", "file" or "missing"). Never logged."""
    key = environ.get(KEY_VAR, "").strip()
    if key:
        return key, "env"
    path = environ.get(ENV_FILE_VAR, "").strip()
    if not path:
        return None, "missing"
    file = Path(path).expanduser()
    if not file.is_file():
        raise PilotError(f"{ENV_FILE_VAR} does not name a readable file")
    if os.name == "posix" and file.stat().st_mode & 0o077:
        raise PilotError(f"{ENV_FILE_VAR} file must not be readable by group or others (chmod 600)")
    for raw in file.read_text().splitlines():
        line = raw.strip()
        if line.startswith("export "):
            line = line[len("export "):].lstrip()
        name, sep, value = line.partition("=")
        if sep and name.strip() == KEY_VAR:
            value = value.strip().strip('"').strip("'")
            if value:
                return value, "file"
    raise PilotError(f"{ENV_FILE_VAR} file has no {KEY_VAR} line")


def scrub_tree(directory: Path, secret: str | None) -> int:
    """Replace the secret in every regular file under `directory`; returns files changed."""
    if not secret or not directory.exists():
        return 0
    needle, changed = secret.encode(), 0
    for path in directory.rglob("*"):
        if not path.is_file() or path.is_symlink():
            continue
        data = path.read_bytes()
        if needle in data:
            path.write_bytes(data.replace(needle, REDACTED.encode()))
            changed += 1
    return changed


def require_spend_confirmation(args: argparse.Namespace, key: str | None) -> None:
    if not args.confirm_spend:
        raise PilotError("paid trials refused: pass --confirm-spend after reviewing --dry-run and --estimate")
    if not key:
        raise PilotError(f"paid trials refused: set {KEY_VAR} or {ENV_FILE_VAR}")


# ---------------------------------------------------------------- paths and environments


SCOPES = ("scored", "smoke")


@dataclass
class Layout:
    """Private run directory. Candidates' sandboxed commands cannot read records,
    grader output or the other harness's state (see `*_read_denials`)."""

    root: Path

    def harness(self, harness: str) -> Path:
        return self.root / harness

    def home(self, harness: str) -> Path:
        return self.harness(harness) / "home"

    @property
    def claude_config_dir(self) -> Path:
        return self.harness("claude-code") / "config"

    @property
    def claude_settings(self) -> Path:
        return self.claude_config_dir / "settings.json"

    @property
    def cyber_home(self) -> Path:
        return self.harness("cyber") / "cyber-home"

    @property
    def cyber_config_dir(self) -> Path:
        return self.harness("cyber") / "config"

    @property
    def catalog(self) -> Path:
        return self.harness("cyber") / "catalog" / "models.json"

    def tmp(self, harness: str, task: str) -> Path:
        return self.harness(harness) / "tmp" / task

    def trial_dir(self, scope: str, harness: str, suite: str, task: str) -> Path:
        return self.root / scope / harness / suite / task

    def workspace(self, scope: str, suite: str, task: str) -> Path:
        return self.root / "workspaces" / "claude-code" / scope / suite / task

    def result(self, scope: str, harness: str, suite: str, task: str) -> Path:
        return self.root / scope / "results" / harness / suite / f"{task}-1.json"


def default_cyber_paths() -> tuple[Path, Path]:
    target = Path(os.environ.get("CARGO_TARGET_DIR") or ROOT / "target").expanduser()
    return target / "release" / "cyber", target / "release" / "cyber-sandbox-exec"


def resolve_binaries(config: dict, args: argparse.Namespace) -> dict[str, Path | None]:
    cyber_default, helper_default = default_cyber_paths()
    claude = args.claude_bin or config["claude_code"]["binary"]
    found = shutil.which(claude) if os.sep not in claude else claude
    return {
        "cyber": Path(args.cyber_bin or config["cyber"]["binary"] or cyber_default).expanduser(),
        "sandbox_helper": Path(args.sandbox_helper or config["cyber"]["sandbox_helper"] or helper_default).expanduser(),
        "claude": Path(found).resolve() if found else None,
        "python3": Path(args.python or shutil.which("python3") or "python3"),
        "node": Path(shutil.which("node") or "node"),
        "git": Path(shutil.which("git") or "git"),
        "bash": Path(shutil.which("bash") or "bash"),
    }


def tool_path(binaries: dict[str, Path | None]) -> str:
    """PATH with the selected interpreters first, then the system directories only."""
    dirs: list[str] = []
    for name in ("python3", "node", "git", "bash"):
        binary = binaries.get(name)
        if binary and binary.parent.is_dir() and str(binary.parent) not in dirs:
            dirs.append(str(binary.parent))
    for system in ("/usr/bin", "/bin", "/usr/sbin", "/sbin"):
        if system not in dirs:
            dirs.append(system)
    return os.pathsep.join(dirs)


def base_env(layout: Layout, harness: str, binaries: dict, tmp: Path) -> dict[str, str]:
    """A fresh environment: nothing inherited except locale."""
    return {
        "PATH": tool_path(binaries),
        "HOME": str(layout.home(harness)),
        "TMPDIR": str(tmp),
        "LANG": os.environ.get("LANG", "en_US.UTF-8"),
        "TERM": "dumb",
        "NO_COLOR": "1",
    }


def cyber_env(layout: Layout, binaries: dict, tmp: Path, key: str | None) -> dict[str, str]:
    env = base_env(layout, "cyber", binaries, tmp)
    env.update(
        {
            "CYBER_HOME": str(layout.cyber_home),
            "CYBER_CONFIG_DIR": str(layout.cyber_config_dir),
            "CYBER_DISABLE_PROJECT_CONFIG": "1",
            "CYBER_PURE": "1",
            "CYBER_DISABLE_MEMORY": "1",
            "CYBER_DISABLE_AUTOUPDATE": "1",
            "CYBER_SANDBOX_HELPER": str(binaries["sandbox_helper"]),
        }
    )
    if layout.catalog.is_file():
        env["CYBER_MODELS_PATH"] = str(layout.catalog)
    if key:
        env[KEY_VAR] = key
    return env


def claude_env(config: dict, layout: Layout, binaries: dict, tmp: Path, key: str | None) -> dict[str, str]:
    env = base_env(layout, "claude-code", binaries, tmp)
    env["CLAUDE_CONFIG_DIR"] = str(layout.claude_config_dir)
    env.update(config["claude_code"]["env"])
    for name in config["claude_code"]["model_pin_env"]:
        env[name] = config["model"]["id"]
    env["CLAUDE_CODE_EFFORT_LEVEL"] = config["model"]["effort"]
    if key:
        env[KEY_VAR] = key
    return env


def grader_env(layout: Layout, binaries: dict, tmp: Path) -> dict[str, str]:
    """Graders never see the key or the candidates' configuration."""
    return base_env(layout, "grader", binaries, tmp)


def public_env(env: dict[str, str]) -> dict[str, str]:
    return {k: (REDACTED if k == KEY_VAR else v) for k, v in sorted(env.items())}


# ---------------------------------------------------------------- generated configuration


def shared_read_denials() -> list[Path]:
    """Hidden graders, references, earlier results and personal agent state (paths only;
    nothing under the operator's home is read or copied)."""
    real_home = Path.home()
    return [
        ROOT / "eval" / "graders",
        ROOT / "eval" / "solutions",
        ROOT / "eval" / "results",
        real_home / ".claude",
        real_home / ".claude.json",
        real_home / ".config" / "cyber",
        real_home / ".local" / "share" / "cyber",
    ]


def claude_read_denials(layout: Layout) -> list[str]:
    """Directories Claude Code's sandboxed commands must not read."""
    own = [layout.root / scope for scope in SCOPES]
    own += [layout.harness("cyber"), layout.root / "report", layout.claude_config_dir / "projects"]
    return [str(p) for p in shared_read_denials() + own]


def cyber_read_denials(layout: Layout) -> list[str]:
    """`sandbox.deny_read` globs for Cyber: the same private paths, the other harness's
    state, and every grading record (Cyber keeps its own workspaces under its output)."""
    paths = shared_read_denials() + [layout.harness("claude-code"), layout.root / "workspaces", layout.root / "report"]
    globs = [f"{p}/**" for p in paths] + [str(Path.home() / ".claude.json")]
    for scope in SCOPES:
        base = layout.root / scope
        globs += [f"{base}/claude-code/**", f"{base}/results/**"]
        globs += [f"{base}/cyber/**/{name}" for name in ("report.json", "report.md", "trials/**", "*.log", "command.json")]
    return globs


def cyber_config(config: dict, layout: Layout) -> dict:
    generated = json.loads(json.dumps(config["cyber"]["config"]))
    ref = config["model"]["cyber_ref"]
    generated["model"] = ref
    generated["model_roles"] = {"default": ref, "small": ref}
    generated.setdefault("sandbox", {})["deny_read"] = cyber_read_denials(layout)
    return generated


def claude_settings(config: dict, layout: Layout) -> dict:
    settings = json.loads(json.dumps(config["claude_code"]["settings"]))
    settings["model"] = config["model"]["id"]
    settings["effortLevel"] = config["model"]["effort"]
    sandbox = settings.setdefault("sandbox", {})
    sandbox.setdefault("filesystem", {})["denyRead"] = claude_read_denials(layout)
    return settings


def write_isolated_config(config: dict, layout: Layout) -> None:
    for harness in HARNESSES:
        layout.home(harness).mkdir(parents=True, exist_ok=True)
    layout.home("grader").mkdir(parents=True, exist_ok=True)
    layout.claude_config_dir.mkdir(parents=True, exist_ok=True)
    layout.cyber_config_dir.mkdir(parents=True, exist_ok=True)
    layout.cyber_home.mkdir(parents=True, exist_ok=True)
    layout.claude_settings.write_text(json.dumps(claude_settings(config, layout), indent=2) + "\n")
    (layout.cyber_config_dir / "cyber.json").write_text(json.dumps(cyber_config(config, layout), indent=2) + "\n")


def claude_command(config: dict, binaries: dict, layout: Layout, task: Task) -> list[str]:
    cc = config["claude_code"]
    cmd = [str(binaries["claude"])]
    if cc["bare"]:
        cmd.append("--bare")
    cmd += [
        "--print",
        "--output-format", "stream-json",
        "--verbose",
        "--model", config["model"]["id"],
        "--effort", config["model"]["effort"],
        "--permission-mode", cc["permission_mode"],
        "--permission-prompts", "none",
        "--settings", str(layout.claude_settings),
        "--setting-sources", "user",
        "--strict-mcp-config",
        "--disallowedTools", ",".join(cc["disallowed_tools"]),
        "--max-budget-usd", f"{task.budget_usd:g}",
        task.prompt,
    ]
    return cmd


def cyber_command(config: dict, binaries: dict, task: Task, output: Path) -> list[str]:
    return [
        str(binaries["cyber"]), "eval", "run", str(task.manifest),
        "--model", config["model"]["cyber_ref"],
        "--task", task.id,
        "--trials", "1", "--trial-start", "1", "--jobs", "1",
        "--output", str(output), "--keep-workspaces",
    ]


# ---------------------------------------------------------------- dry-run checks


def run_quiet(cmd: list[str], env: dict[str, str] | None = None, timeout: int = 60, cwd: Path | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, env=env, cwd=cwd, capture_output=True, text=True, timeout=timeout, stdin=subprocess.DEVNULL)


def version_tuple(text: str) -> tuple[int, ...]:
    match = re.search(r"(\d+)\.(\d+)(?:\.(\d+))?", text)
    return tuple(int(p) for p in match.groups() if p is not None) if match else ()


def check_inputs(config: dict, tasks: list[Task]) -> list[Check]:
    checks = []
    for rel in config["suites"]:
        suite_tasks = [t for t in tasks if t.manifest == ROOT / rel]
        expected = config["expected_tasks_per_suite"]
        checks.append(Check(f"manifest {rel}", len(suite_tasks) == expected, f"{len(suite_tasks)} tasks, expected {expected}"))
    bad = [t.id for t in tasks if graders.tree_sha256(t.fixture) != t.tree_sha256]
    checks.append(Check("fixture tree_sha256", not bad, f"mismatch: {bad}" if bad else f"{len(tasks)} fixtures match"))
    missing = [t.id for t in tasks if not (t.grader_dir / "grade.py").is_file()]
    checks.append(Check("hidden graders present", not missing, f"missing: {missing}" if missing else ""))
    missing = [t.id for t in tasks if not (ROOT / "eval" / "solutions" / t.id).is_dir()]
    checks.append(Check("reference solutions present", not missing, f"missing: {missing}" if missing else ""))
    agent_files = [
        str(p.relative_to(ROOT)) for t in tasks for p in t.fixture.rglob("*")
        if p.name in {"CLAUDE.md", "AGENTS.md", ".claude", ".cyber", ".mcp.json", "CYBER.md"}
    ]
    checks.append(Check("fixtures carry no agent instructions/config", not agent_files, ", ".join(agent_files)))
    dashed = [t.id for t in tasks if t.prompt.lstrip().startswith("-")]
    checks.append(Check("prompts parse as a positional argument", not dashed, ", ".join(dashed)))
    return checks


def check_binaries(config: dict, binaries: dict, layout: Layout) -> list[Check]:
    checks = []
    for name in ("cyber", "sandbox_helper", "claude"):
        path = binaries[name]
        ok = bool(path) and path.is_file() and os.access(path, os.X_OK)
        checks.append(Check(f"binary {name}", ok, str(path) if ok else f"not found: {path}"))
    if binaries["claude"] and binaries["claude"].is_file():
        env = claude_env(config, layout, binaries, layout.tmp("claude-code", "dry-run"), None)
        layout.tmp("claude-code", "dry-run").mkdir(parents=True, exist_ok=True)
        out = run_quiet([str(binaries["claude"]), "--version"], env=env).stdout.strip()
        want = config["claude_code"]["version"]
        checks.append(Check("claude version", out.startswith(want), f"{out!r}, pinned {want}"))
        help_text = run_quiet([str(binaries["claude"]), "--help"], env=env).stdout
        absent = [f for f in config["claude_code"]["required_flags"] if f not in help_text]
        checks.append(Check("claude flags", not absent, f"missing: {absent}" if absent else f"{len(config['claude_code']['required_flags'])} flags present"))
    if binaries["cyber"] and binaries["cyber"].is_file():
        help_text = run_quiet([str(binaries["cyber"]), "eval", "run", "--help"]).stdout
        absent = [f for f in config["cyber"]["eval_flags"] if f not in help_text]
        checks.append(Check("cyber eval flags", not absent, f"missing: {absent}" if absent else ""))
        checks.append(Check("cyber version", True, run_quiet([str(binaries["cyber"]), "--version"]).stdout.strip()))
    return checks


def check_runtimes(config: dict, binaries: dict) -> list[Check]:
    checks = []
    py = run_quiet([str(binaries["python3"]), "--version"]).stdout.strip()
    want = version_tuple(config["runtime"]["python_min"])
    checks.append(Check("python3", version_tuple(py) >= want, f"{py} at {binaries['python3']}, need >= {config['runtime']['python_min']}"))
    node = run_quiet([str(binaries["node"]), "--version"]).stdout.strip()
    major = config["runtime"]["node_major"]
    checks.append(Check("node", version_tuple(node)[:1] == (major,), f"{node}, need {major}.x"))
    git = run_quiet([str(binaries["git"]), "--version"]).stdout.strip()
    checks.append(Check("git", bool(git), git))
    issue = graders.atomic_mkdir_issue()
    checks.append(Check("mkdir lock contract", issue is None, issue or ""))
    return checks


def check_sandbox_support() -> list[Check]:
    system = platform.system()
    if system == "Darwin":
        tool = "/usr/bin/sandbox-exec"
        return [Check("OS sandbox (Claude Code bash, Cyber commands)", Path(tool).is_file(), tool)]
    if system == "Linux":
        tool = shutil.which("bwrap")
        return [Check("bubblewrap for Claude Code sandbox", bool(tool), tool or "bwrap not found")]
    return [Check("supported platform", False, f"{system}: Windows is excluded (no comparable sandbox)")]


def check_run_root(layout: Layout) -> list[Check]:
    real_home = Path.home().resolve()
    root = layout.root.resolve()
    return [
        Check("run root outside repository", not root.is_relative_to(ROOT.resolve()), str(root)),
        Check("run root outside personal ~/.claude", not root.is_relative_to(real_home / ".claude"), ""),
        Check("CLAUDE_CONFIG_DIR is isolated", layout.claude_config_dir.resolve() != real_home / ".claude", str(layout.claude_config_dir)),
        Check("HOME is isolated for both harnesses", all(layout.home(h).resolve() != real_home for h in HARNESSES), ""),
    ]


def check_isolation(config: dict, layout: Layout) -> list[Check]:
    checks = check_run_root(layout)
    personal = [p.name for p in layout.claude_config_dir.iterdir() if p.name != "settings.json"] if layout.claude_config_dir.is_dir() else []
    checks.append(Check("isolated Claude config holds only generated settings", not personal, ", ".join(personal), warning=True))
    settings = json.loads(layout.claude_settings.read_text())
    sandbox = settings.get("sandbox", {})
    network = sandbox.get("network", {})
    checks.append(Check(
        "Claude Code command network off",
        sandbox.get("enabled") is True and network.get("allowedDomains") == [] and network.get("strictAllowlist") is True
        and sandbox.get("allowUnsandboxedCommands") is False and sandbox.get("failIfUnavailable") is True,
        "sandbox on, empty strict allowlist, no unsandboxed retry",
    ))
    cyber = json.loads((layout.cyber_config_dir / "cyber.json").read_text())
    checks.append(Check("Cyber command network off", cyber.get("sandbox", {}).get("network") == "off", ""))
    managed = [p for p in config["claude_code"]["managed_settings_paths"] if Path(p).exists()]
    checks.append(Check("no host-managed Claude Code settings", not managed, f"present (not read): {managed}" if managed else "", warning=True))
    return checks


def check_cyber_runtime(config: dict, binaries: dict, layout: Layout) -> list[Check]:
    """Resolve the catalog and configuration under the isolated environment; no inference."""
    if not (binaries["cyber"] and binaries["cyber"].is_file()):
        return []
    tmp = layout.tmp("cyber", "dry-run")
    tmp.mkdir(parents=True, exist_ok=True)
    env = cyber_env(layout, binaries, tmp, None)
    checks = []
    if not layout.catalog.is_file():
        checks.append(pin_catalog(binaries, layout, env))
        env = cyber_env(layout, binaries, tmp, None)
    listing = run_quiet([str(binaries["cyber"]), "models", "anthropic", "--verbose"], env=env, cwd=tmp)
    model = config["model"]["id"]
    checks.append(Check(f"Cyber catalog lists {model}", model in listing.stdout, "pinned catalog" if layout.catalog.is_file() else listing.stderr.strip()[:200]))
    resolved = run_quiet([str(binaries["cyber"]), "debug", "config"], env=env, cwd=tmp)
    text = resolved.stdout
    ok = resolved.returncode == 0 and '"network": "off"' in text.replace("'", '"') and config["model"]["cyber_ref"] in text
    checks.append(Check("Cyber resolved config (isolated)", ok, "sandbox.network off, model pinned" if ok else (resolved.stderr or text)[:300]))
    probe = run_quiet([str(binaries["cyber"]), "sandbox", "test"], env=env, cwd=tmp)
    network = [line for line in probe.stdout.splitlines() if " network " in line]
    checks.append(Check("Cyber sandbox probes (read, write, network denied)", probe.returncode == 0 and bool(network),
                        network[0].split("  ")[-1] if network else probe.stderr.strip()[:200]))
    return checks


def pin_catalog(binaries: dict, layout: Layout, env: dict[str, str]) -> Check:
    """Fetch the public models.dev catalog once through Cyber and pin it for every trial."""
    run_quiet([str(binaries["cyber"]), "models", "refresh"], env=env, cwd=Path(env["TMPDIR"]), timeout=60)
    cache = layout.cyber_home / "cache" / "models.json"
    if not cache.is_file():
        return Check("Cyber model catalog pinned", False, "catalog fetch failed; set CYBER_MODELS_PATH source manually")
    layout.catalog.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(cache, layout.catalog)
    return Check("Cyber model catalog pinned", True, f"sha256 {sha256_file(layout.catalog)[:16]}")


def check_catalog_prices(config: dict, layout: Layout) -> list[Check]:
    if not layout.catalog.is_file():
        return []
    models = json.loads(layout.catalog.read_text()).get("anthropic", {}).get("models", {})
    entry = models.get(config["model"]["id"])
    if not entry:
        return [Check("catalog price agreement", False, "model missing from pinned catalog")]
    want = config["model"]["prices_usd_per_mtok"]
    got = {k: entry.get("cost", {}).get(k) for k in want}
    efforts = [v for o in entry.get("reasoning_options", []) if o.get("type") == "effort" for v in o.get("values", [])]
    return [
        Check("catalog price agreement", got == want, f"catalog {got}"),
        Check("effort level supported", config["model"]["effort"] in efforts, f"{efforts}"),
    ]


def check_key(environ: dict[str, str]) -> Check:
    try:
        key, source = load_api_key(environ)
    except PilotError as error:
        return Check("Anthropic API key", False, str(error))
    if not key:
        return Check("Anthropic API key", False, f"not provided ({KEY_VAR} or {ENV_FILE_VAR}); required only for paid modes", warning=True)
    return Check("Anthropic API key", True, f"present via {source} (value never shown)")


def check_disk(layout: Layout, minimum_gib: float = 5.0) -> Check:
    free = shutil.disk_usage(layout.root).free / 2**30
    return Check("free disk at run root", free >= minimum_gib, f"{free:.1f} GiB")


def run_grader_controls(binaries: dict) -> Check:
    proc = run_quiet([str(binaries["python3"]), str(ROOT / "scripts" / "check_eval_graders.py")], timeout=3600)
    tail = (proc.stdout + proc.stderr).strip().splitlines()[-1:] or [""]
    return Check("grader controls (broken FAIL / reference PASS)", proc.returncode == 0, tail[0][:200])


def dry_run(config: dict, args: argparse.Namespace, layout: Layout, tasks: list[Task]) -> list[Check]:
    binaries = resolve_binaries(config, args)
    layout.root.mkdir(parents=True, exist_ok=True)
    write_isolated_config(config, layout)
    checks = check_inputs(config, tasks)
    checks += check_binaries(config, binaries, layout)
    checks += check_runtimes(config, binaries)
    checks += check_sandbox_support()
    checks += check_isolation(config, layout)
    checks += check_cyber_runtime(config, binaries, layout)
    checks += check_catalog_prices(config, layout)
    checks.append(check_key(dict(os.environ)))
    checks.append(check_disk(layout))
    if args.grader_controls:
        checks.append(run_grader_controls(binaries))
    sample = tasks[0]
    preview = {
        "plan": frozen_plan(tasks),
        "claude_command": claude_command(config, binaries, layout, sample)[:-1] + ["<task prompt>"],
        "cyber_command": cyber_command(config, binaries, sample, layout.trial_dir("scored", "cyber", sample.suite, sample.id)),
        "claude_env": public_env(claude_env(config, layout, binaries, layout.tmp("claude-code", sample.id), None)),
        "cyber_env": public_env(cyber_env(layout, binaries, layout.tmp("cyber", sample.id), None)),
    }
    (layout.root / "dry-run.json").write_text(json.dumps(
        {"checks": [c.__dict__ for c in checks], "preview": preview}, indent=2, default=str) + "\n")
    return checks


# ---------------------------------------------------------------- cost estimate


def per_trial_tokens(report: Path) -> dict[str, float]:
    summary = json.loads((ROOT / report).read_text())["summary"]
    return {k: summary["tokens"].get(k, 0) / summary["trials"] for k in TOKEN_CLASSES}


def price(tokens: dict[str, float], prices: dict[str, float]) -> float:
    return sum(tokens[k] * prices[k] for k in TOKEN_CLASSES) / 1e6


def estimate(config: dict, tasks: list[Task]) -> dict:
    refs = config["reference_reports"]
    prices = config["model"]["prices_usd_per_mtok"]
    hard = per_trial_tokens(refs["hard"])
    std_run, hard_run = (json.loads((ROOT / p).read_text())["summary"]["tokens"] for p in refs["standard_hard_ratio_source"])
    in_ratio = (std_run["input"] + std_run["cache_read"]) / (hard_run["input"] + hard_run["cache_read"])
    out_ratio = std_run["output"] / hard_run["output"]
    standard = {k: hard[k] * (out_ratio if k == "output" else in_ratio) for k in TOKEN_CLASSES}
    standard_upper = per_trial_tokens(refs["standard_upper"])
    counts = {s: sum(1 for t in tasks if t.suite == s) for s in {t.suite for t in tasks}}
    hard_suite = next(s for s in counts if "hard" in s)
    std_suite = next(s for s in counts if "hard" not in s)
    per_harness = price(hard, prices) * counts[hard_suite] + price(standard, prices) * counts[std_suite]
    per_harness_upper = price(hard, prices) * counts[hard_suite] + price(standard_upper, prices) * counts[std_suite]
    caps = sum(t.budget_usd for t in tasks)
    return {
        "prices_usd_per_mtok": prices,
        "hard_per_trial_tokens": hard,
        "standard_per_trial_tokens": standard,
        "standard_upper_per_trial_tokens": standard_upper,
        "standard_to_hard_ratio": {"input_and_cache": in_ratio, "output": out_ratio},
        "hard_per_trial_usd": price(hard, prices),
        "standard_per_trial_usd": price(standard, prices),
        "standard_upper_per_trial_usd": price(standard_upper, prices),
        "per_harness_usd": per_harness,
        "per_harness_upper_usd": per_harness_upper,
        "both_harnesses_usd": 2 * per_harness,
        "both_harnesses_upper_usd": 2 * per_harness_upper,
        "budget_cap_per_harness_usd": caps,
        "budget_cap_both_usd": 2 * caps,
        "max_total_usd_guard": config["max_total_usd"],
    }


def print_estimate(est: dict) -> None:
    p = est["prices_usd_per_mtok"]
    print(f"Prices per MTok: input ${p['input']}, output ${p['output']}, cache read ${p['cache_read']}, cache write ${p['cache_write']}")
    for label, key in (("hard", "hard"), ("standard", "standard"), ("standard (upper)", "standard_upper")):
        t = est[f"{key}_per_trial_tokens"]
        terms = " + ".join(f"{t[k]:,.0f} {k} x ${p[k]}" for k in TOKEN_CLASSES)
        print(f"{label:17} per trial: ({terms}) / 1e6 = ${est[f'{key}_per_trial_usd']:.3f}")
    r = est["standard_to_hard_ratio"]
    print(f"standard/hard token ratio (GPT-6.1 Sol Cyber pilot): input+cache {r['input_and_cache']:.3f}, output {r['output']:.3f}")
    print(f"per harness: 20 x hard + 20 x standard = ${est['per_harness_usd']:.2f} (upper ${est['per_harness_upper_usd']:.2f})")
    print(f"both harnesses: ${est['both_harnesses_usd']:.2f} (upper ${est['both_harnesses_upper_usd']:.2f})")
    print(f"manifest budget caps: ${est['budget_cap_per_harness_usd']:.2f} per harness, ${est['budget_cap_both_usd']:.2f} both;"
          f" run stops scheduling at --max-total-usd ${est['max_total_usd_guard']:.2f}")


# ---------------------------------------------------------------- trials


def stop_group(proc: subprocess.Popen) -> None:
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        proc.wait(timeout=10)


def run_logged(cmd: list[str], env: dict, cwd: Path, stdout: Path, stderr: Path, timeout: int) -> tuple[int | None, bool, float]:
    begin = time.monotonic()
    with open(stdout, "w") as out, open(stderr, "w") as err:
        proc = subprocess.Popen(cmd, env=env, cwd=cwd, stdin=subprocess.DEVNULL, stdout=out, stderr=err, start_new_session=True)
        try:
            return proc.wait(timeout=timeout), False, time.monotonic() - begin
        except subprocess.TimeoutExpired:
            stop_group(proc)
            return proc.returncode, True, time.monotonic() - begin


def parse_stream(path: Path) -> list[dict]:
    events = []
    for line in path.read_text(errors="replace").splitlines():
        try:
            events.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return events


def claude_usage(result: dict) -> tuple[dict, list[str]]:
    """Token classes summed over every model Claude Code reports, and the model names."""
    per_model = result.get("modelUsage") or {}
    if per_model:
        tokens = {
            "input": sum(m.get("inputTokens", 0) for m in per_model.values()),
            "output": sum(m.get("outputTokens", 0) for m in per_model.values()),
            "cache_read": sum(m.get("cacheReadInputTokens", 0) for m in per_model.values()),
            "cache_write": sum(m.get("cacheCreationInputTokens", 0) for m in per_model.values()),
        }
        return tokens, sorted(per_model)
    usage = result.get("usage") or {}
    return {
        "input": usage.get("input_tokens", 0),
        "output": usage.get("output_tokens", 0),
        "cache_read": usage.get("cache_read_input_tokens", 0),
        "cache_write": usage.get("cache_creation_input_tokens", 0),
    }, []


def classify_claude(code: int | None, timed_out: bool, result: dict | None) -> tuple[bool, str]:
    """(infrastructure_failure, stop_reason). Budget stops and timeouts are graded."""
    if timed_out:
        return False, "timeout"
    if result is None:
        return True, "error"
    subtype = result.get("subtype", "")
    if subtype == "success" and not result.get("is_error"):
        return False, "end_turn"
    if "budget" in subtype:
        return False, "budget"
    if "max_turns" in subtype:
        return False, "max_turns"
    return True, subtype or "error"


def grade(task: Task, workspace: Path, env: dict, trial_dir: Path) -> dict:
    cmd = [a.replace("{grader_dir}", str(task.grader_dir)).replace("{workspace}", str(workspace)) for a in task.grade_command]
    cmd[0] = shutil.which(cmd[0], path=env["PATH"]) or cmd[0]
    try:
        proc = subprocess.run(cmd, env=env, cwd=ROOT, capture_output=True, text=True, timeout=task.timeout)
    except subprocess.TimeoutExpired:
        return {"passed": False, "grader_exit": None, "grader_tail": "grader timeout", "grader_infrastructure": True}
    text = proc.stdout + proc.stderr
    (trial_dir / "grader.log").write_text(text)
    return {"passed": proc.returncode == 0, "grader_exit": proc.returncode, "grader_tail": text[-4000:]}


def run_claude_trial(ctx: "RunContext", task: Task) -> dict:
    layout, config = ctx.layout, ctx.config
    workspace = layout.workspace(ctx.scope, task.suite, task.id)
    trial_dir = layout.trial_dir(ctx.scope, "claude-code", task.suite, task.id)
    trial_dir.mkdir(parents=True, exist_ok=True)
    tmp = layout.tmp("claude-code", task.id)
    tmp.mkdir(parents=True, exist_ok=True)
    graders.prepare_workspace(graders.Task("", task.id, task.fixture, task.tree_sha256, task.grader_dir, task.grade_command, task.timeout), workspace)
    cmd = claude_command(config, ctx.binaries, layout, task)
    env = claude_env(config, layout, ctx.binaries, tmp, ctx.key)
    (trial_dir / "command.json").write_text(json.dumps({"argv": cmd, "env": public_env(env)}, indent=2) + "\n")
    code, timed_out, latency = run_logged(cmd, env, workspace, trial_dir / "events.jsonl", trial_dir / "stderr.log", task.timeout)
    events = parse_stream(trial_dir / "events.jsonl")
    result = next((e for e in reversed(events) if e.get("type") == "result"), None)
    infra, stop = classify_claude(code, timed_out, result)
    tokens, models = claude_usage(result or {})
    record = {
        "harness": "claude-code", "suite": task.suite, "task": task.id, "trial": 1,
        "passed": False, "infrastructure_failure": infra, "stop_reason": stop, "exit_code": code,
        "latency_seconds": latency, "native_duration_ms": (result or {}).get("duration_ms"),
        "turns": (result or {}).get("num_turns"), "tokens": tokens, "models_used": models,
        "foreign_model_usage": [m for m in models if m != config["model"]["id"]],
        "native_cost_usd": (result or {}).get("total_cost_usd"), "computed_cost_usd": price(tokens, config["model"]["prices_usd_per_mtok"]),
        "permission_denials": len((result or {}).get("permission_denials") or []),
        "api_retries": sum(1 for e in events if e.get("subtype") == "api_retry"),
        "error": None if not infra else ((result or {}).get("result") or "no result event")[:500],
        "grader_exit": None, "grader_tail": "",
    }
    if not infra:
        record.update(grade(task, workspace, grader_env(layout, ctx.binaries, tmp), trial_dir))
        record["infrastructure_failure"] = bool(record.pop("grader_infrastructure", False))
    return record


def run_cyber_trial(ctx: "RunContext", task: Task) -> dict:
    layout = ctx.layout
    output = layout.trial_dir(ctx.scope, "cyber", task.suite, task.id)
    output.mkdir(parents=True, exist_ok=True)
    tmp = layout.tmp("cyber", task.id)
    tmp.mkdir(parents=True, exist_ok=True)
    cmd = cyber_command(ctx.config, ctx.binaries, task, output)
    env = cyber_env(layout, ctx.binaries, tmp, ctx.key)
    (output / "command.json").write_text(json.dumps({"argv": cmd, "env": public_env(env)}, indent=2) + "\n")
    code, timed_out, latency = run_logged(cmd, env, tmp, output / "run.log", output / "stderr.log", task.timeout + 300)
    report = output / "report.json"
    trial = json.loads(report.read_text())["trials"][0] if report.is_file() else None
    if trial is None:
        return {"harness": "cyber", "suite": task.suite, "task": task.id, "trial": 1, "passed": False,
                "infrastructure_failure": True, "stop_reason": "timeout" if timed_out else "error", "exit_code": code,
                "latency_seconds": latency, "tokens": {k: 0 for k in TOKEN_CLASSES}, "native_cost_usd": None,
                "computed_cost_usd": 0.0, "error": "no Cyber report", "grader_exit": None, "grader_tail": ""}
    tokens = {k: trial["tokens"].get(k, 0) for k in TOKEN_CLASSES}
    return {
        "harness": "cyber", "suite": task.suite, "task": task.id, "trial": 1,
        "passed": trial["passed"], "infrastructure_failure": trial["infrastructure_failure"],
        "stop_reason": trial["stop_reason"], "exit_code": code, "latency_seconds": trial["latency_seconds"],
        "process_seconds": latency, "turns": trial.get("turns"), "tokens": tokens,
        "reasoning_tokens": trial["tokens"].get("reasoning"), "native_cost_usd": trial.get("cost_usd"),
        "computed_cost_usd": price(tokens, ctx.config["model"]["prices_usd_per_mtok"]),
        "error": trial.get("error"), "grader_exit": trial.get("grader_exit"), "grader_tail": trial.get("grader_tail", ""),
    }


@dataclass
class RunContext:
    config: dict
    layout: Layout
    binaries: dict
    key: str
    scope: str
    max_total_usd: float
    results: list[dict] = field(default_factory=list)

    def spent(self) -> float:
        return sum(r.get("native_cost_usd") or r.get("computed_cost_usd") or 0.0 for r in self.results)


def result_path(ctx: RunContext, harness: str, task: Task) -> Path:
    return ctx.layout.result(ctx.scope, harness, task.suite, task.id)


RUNNERS = {"cyber": "run_cyber_trial", "claude-code": "run_claude_trial"}


def run_one(ctx: RunContext, harness: str, task: Task) -> tuple[dict, bool]:
    """Run one trial, or load it when resuming; persist and scrub a new record.
    The flag is true for a trial run now."""
    path = result_path(ctx, harness, task)
    if path.is_file():
        return json.loads(path.read_text()), False
    record = globals()[RUNNERS[harness]](ctx, task)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(record, indent=2) + "\n")
    for directory in (ctx.layout.root / ctx.scope, ctx.layout.root / "workspaces"):
        scrub_tree(directory, ctx.key)
    status = "INFRA" if record["infrastructure_failure"] else "PASS" if record["passed"] else "FAIL"
    print(f"{task.suite} {task.id} [{harness}]: {status} ({record['latency_seconds']:.0f}s)", flush=True)
    return record, True


def stop_reason(ctx: RunContext, harness: str, task: Task) -> int:
    """0 to continue; 3 when the spend guard is reached before a new trial."""
    if result_path(ctx, harness, task).is_file() or ctx.spent() < ctx.max_total_usd:
        return 0
    print(f"Stopping: reported spend ${ctx.spent():.2f} reached --max-total-usd ${ctx.max_total_usd:.2f}", flush=True)
    return 3


def early_infrastructure_failure(ctx: RunContext, record: dict) -> bool:
    attempts = [r for r in ctx.results if r.get("harness") == record["harness"]]
    return record["infrastructure_failure"] and len(attempts) <= 2


def execute(ctx: RunContext, tasks: list[Task], plan: list[dict]) -> int:
    by_key = {(t.suite, t.id): t for t in tasks}
    for step in plan:
        task = by_key[(step["suite"], step["task"])]
        for harness in step["order"]:
            status = stop_reason(ctx, harness, task)
            if status:
                return status
            record, fresh = run_one(ctx, harness, task)
            ctx.results.append(record)
            if fresh and early_infrastructure_failure(ctx, record):
                print("Stopping on early infrastructure failure; preserve evidence before continuing (rerun resumes).", flush=True)
                return 2
    print(f"Done: reported spend ${ctx.spent():.2f}", flush=True)
    return 0


# ---------------------------------------------------------------- reports and provenance


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def input_hashes(config: dict, tasks: list[Task]) -> dict[str, str]:
    paths = [ROOT / rel for rel in config["suites"]]
    for t in tasks:
        for directory in (t.fixture, t.grader_dir, ROOT / "eval" / "solutions" / t.id):
            paths += [p for p in directory.rglob("*") if p.is_file() and not graders.SKIPPED_NAMES.intersection(p.parts)]
    return {str(p.relative_to(ROOT)): sha256_file(p) for p in sorted(set(paths))}


def binary_hashes(binaries: dict) -> dict[str, str]:
    return {name: sha256_file(path) for name, path in binaries.items() if name in ("cyber", "sandbox_helper", "claude") and path and path.is_file()}


def summarize(results: list[dict]) -> list[dict]:
    rows = []
    for harness in HARNESSES:
        for suite in sorted({r["suite"] for r in results}):
            group = [r for r in results if r["harness"] == harness and r["suite"] == suite]
            if not group:
                continue
            infra = sum(r["infrastructure_failure"] for r in group)
            passed = sum(r["passed"] for r in group)
            rows.append({
                "harness": harness, "suite": suite, "attempted": len(group), "passed": passed,
                "infrastructure_failures": infra, "grader_failures": len(group) - passed - infra,
                "median_native_latency_seconds": statistics.median(r["latency_seconds"] for r in group),
                "tokens": {k: sum(r["tokens"].get(k, 0) for r in group) for k in TOKEN_CLASSES},
                "native_cost_usd": sum(r.get("native_cost_usd") or 0.0 for r in group),
                "computed_cost_usd": sum(r["computed_cost_usd"] for r in group),
            })
    return rows


def write_reports(ctx: RunContext, tasks: list[Task]) -> None:
    out = ctx.layout.root / "report" / ctx.scope
    out.mkdir(parents=True, exist_ok=True)
    (out / "summary.json").write_text(json.dumps({"summaries": summarize(ctx.results)}, indent=2) + "\n")
    columns = ["harness", "suite", "task", "passed", "infrastructure_failure", "stop_reason", "latency_seconds",
               "grader_exit", "input", "output", "cache_read", "cache_write", "native_cost_usd", "computed_cost_usd"]
    with open(out / "task-outcomes.csv", "w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=columns, extrasaction="ignore")
        writer.writeheader()
        for r in ctx.results:
            writer.writerow({**r, **r["tokens"]})
    scrub_tree(out, ctx.key)


def write_provenance(config: dict, layout: Layout, binaries: dict, tasks: list[Task], plan: list[dict]) -> None:
    path = layout.root / "provenance.json"
    if path.is_file():
        return
    claude_version = run_quiet([str(binaries["claude"]), "--version"]).stdout.strip()
    git_sha = run_quiet(["git", "-C", str(ROOT), "rev-parse", "HEAD"]).stdout.strip()
    dirty = bool(run_quiet(["git", "-C", str(ROOT), "status", "--porcelain"]).stdout.strip())
    path.write_text(json.dumps({
        "source_revision": git_sha, "dirty": dirty, "claude_code_version": claude_version,
        "model": config["model"], "plan": plan, "binaries_sha256": binary_hashes(binaries),
        "catalog_sha256": sha256_file(layout.catalog) if layout.catalog.is_file() else None,
        "claude_settings": json.loads(layout.claude_settings.read_text()),
        "cyber_config": json.loads((layout.cyber_config_dir / "cyber.json").read_text()),
        "manifests_sha256": {rel: sha256_file(ROOT / rel) for rel in config["suites"]},
        "eval_input_sha256": input_hashes(config, tasks),
        "host": {"platform": platform.platform(), "machine": platform.machine(), "cpus": os.cpu_count()},
    }, indent=2) + "\n")


def verify(config: dict, layout: Layout, binaries: dict, tasks: list[Task]) -> list[Check]:
    provenance = json.loads((layout.root / "provenance.json").read_text())
    current = input_hashes(config, tasks)
    changed = [p for p, h in provenance["eval_input_sha256"].items() if current.get(p) != h]
    checks = [Check("eval inputs unchanged", not changed, f"{len(current)} files" if not changed else f"changed: {changed[:5]}")]
    checks.append(Check("binaries unchanged", binary_hashes(binaries) == provenance["binaries_sha256"], ""))
    missing = [
        f"{h}:{t.suite}/{t.id}" for t in tasks for h in HARNESSES
        if not layout.result("scored", h, t.suite, t.id).is_file()
    ]
    checks.append(Check("every planned trial recorded", not missing, f"missing {len(missing)}" if missing else f"{2 * len(tasks)} trials"))
    key, _ = load_api_key(dict(os.environ))
    leaked = scrub_tree(layout.root, key) if key else 0
    checks.append(Check("no key material in run directory", leaked == 0, f"{leaked} files scrubbed" if leaked else ""))
    return checks


# ---------------------------------------------------------------- entry point


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    mode = parser.add_mutually_exclusive_group(required=True)
    for flag in ("--dry-run", "--estimate", "--smoke", "--run", "--verify"):
        mode.add_argument(flag, action="store_true")
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--root", type=Path, help="private run directory outside the repository")
    parser.add_argument("--confirm-spend", action="store_true", help="required for --smoke and --run")
    parser.add_argument("--max-total-usd", type=float, help="stop scheduling trials past this reported spend")
    parser.add_argument("--grader-controls", action="store_true", help="dry-run also runs all broken/reference grader controls")
    parser.add_argument("--cyber-bin")
    parser.add_argument("--sandbox-helper")
    parser.add_argument("--claude-bin")
    parser.add_argument("--python", help="python3 >= 3.10 used by tasks and graders")
    args = parser.parse_args(argv)
    if not args.estimate and not args.root:
        parser.error("--root is required")
    return args


def report_checks(checks: list[Check]) -> int:
    for check in checks:
        print(check.line())
    failed = [c for c in checks if not c.ok and not c.warning]
    print(f"{len(checks) - len(failed)}/{len(checks)} checks passed or warned; {len(failed)} failed")
    return 1 if failed else 0


def paid(config: dict, args: argparse.Namespace, layout: Layout, tasks: list[Task]) -> int:
    key, _ = load_api_key(dict(os.environ))
    require_spend_confirmation(args, key)
    checks = dry_run(config, args, layout, tasks)
    if report_checks(checks):
        raise PilotError("dry-run checks failed; no trial started")
    binaries = resolve_binaries(config, args)
    if args.smoke:
        smoke = config["smoke_task"]
        tasks = [t for t in tasks if t.manifest == ROOT / smoke["suite"] and t.id == smoke["task"]]
    plan = frozen_plan(tasks)
    scope = "smoke" if args.smoke else "scored"
    if not args.smoke:
        write_provenance(config, layout, binaries, tasks, plan)
    (layout.root / f"{scope}-plan.json").write_text(json.dumps(plan, indent=2) + "\n")
    ctx = RunContext(config, layout, binaries, key, scope, args.max_total_usd or config["max_total_usd"])
    status = execute(ctx, tasks, plan)
    write_reports(ctx, tasks)
    return status


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    config = load_config(args.config)
    tasks = load_tasks(config)
    if args.estimate:
        print_estimate(estimate(config, tasks))
        return 0
    layout = Layout(args.root.expanduser().resolve())
    try:
        if args.dry_run:
            status = report_checks(dry_run(config, args, layout, tasks))
            print_estimate(estimate(config, tasks))
            return status
        if args.verify:
            return report_checks(verify(config, layout, resolve_binaries(config, args), tasks))
        return paid(config, args, layout, tasks)
    except PilotError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
