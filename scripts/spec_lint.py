#!/usr/bin/env python3
"""Cross-spec consistency lint for openspec/specs.

Checks that shared contracts are declared exactly once by their owning spec and
that every other spec only references declared values:

  * top-level `cyber <command>` names vs. the command tree in cli-commands
  * `/api/v1/<segment>` route groups vs. the route-group list in server-api
  * ID prefixes (`xxx_`) declared by more than one capability
  * requirement names duplicated across capabilities
  * phase dependencies: a (Pn) requirement that references a capability whose
    earliest requirement is a later phase
  * stray ports other than the default 4747 and stray `--output-format`/`--cd`

Exit code 1 on any error. Warnings do not fail the run.
"""
from __future__ import annotations

import re
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPECS = ROOT / "openspec" / "specs"

REQ_RE = re.compile(r"^### Requirement: (.+)$", re.M)
PHASE_RE = re.compile(r"\((P[0-4])\)")
CMD_RE = re.compile(r"`cyber ([a-z][a-z-]*)")
ROUTE_RE = re.compile(r"(?<![\w.])/api/v1/([a-zA-Z_-]+)")  # skip foreign hosts like auth.example/api/v1/...
PREFIX_DECL_RE = re.compile(r"IDs? with prefix `([a-z]{2,5}_)`|\(`([a-z]{2,5}_)` IDs?\)|\ban? `([a-z]{2,5}_)` ID\b")
CODE_RE = re.compile(r"`([a-z][a-z-]+)`")
STALE_PORTS = {"4711", "4096"}  # OpenCode-era defaults; the cyber default is 4747
PORT_RE = re.compile(r":(\d{4,5})\b")

# Values that are flags/placeholders after `cyber`, not commands.
CMD_IGNORE = {"v", "sesions", "__complete", "server"}  # "cyber server listening" is output text
# Route segments that are singletons/streams and legitimately not plural.
ROUTE_SINGLETONS = {"health", "config", "account", "policy", "vcs", "usage", "memory",
                    "lsp", "formatters", "ide", "event", "fs", "messaging", "ws",
                    "openapi"}
# Map route first-segments to their group name in server-api.
ROUTE_TO_GROUP = {
    "sessions": "session", "messages": "message", "permissions": "permission",
    "questions": "question", "agents": "agent", "models": "model", "providers": "provider",
    "credentials": "credential", "oauth-attempts": "oauth", "tools": "tool", "fs": "fs",
    "pty": "pty", "workflows": "workflow", "goals": "goal", "loops": "loop", "jobs": "job",
    "messaging": "messaging", "remote": "remote", "runners": "runner", "routines": "routine",
    "channels": "channel", "worktrees": "worktree", "memory": "memory", "lsp": "lsp",
    "formatters": "formatter", "usage": "usage", "vcs": "vcs", "ide": "ide", "policy": "policy",
    "mcp": "mcp", "skills": "skill", "commands": "command", "hooks": "hook", "plugins": "plugin",
    "config": "config", "account": "account", "event": "event", "health": "health",
    "openapi": "health", "ws": "event", "orgs": "account",
}


def load() -> dict[str, str]:
    return {p.parent.name: p.read_text() for p in sorted(SPECS.glob("*/spec.md"))}


def requirements(text: str) -> list[tuple[str, str, str]]:
    """Return (name, phase, body) per requirement."""
    out = []
    parts = REQ_RE.split(text)
    for i in range(1, len(parts), 2):
        name, body = parts[i].strip(), parts[i + 1]
        m = PHASE_RE.search(body)
        out.append((name, m.group(1) if m else "", body))
    return out


def declared_commands(cli: str) -> set[str]:
    body = cli.split("### Requirement: Command tree", 1)[1].split("### Requirement:", 1)[0]
    names = set(re.findall(r"`([a-z][a-z-]*)`", body))
    return names - {"cyber", "cyber [project]"}


def declared_groups(server: str) -> set[str]:
    body = server.split("### Requirement: Route groups", 1)[1].split("### Requirement:", 1)[0]
    head = body.split("Group names are", 1)[0]
    return set(re.findall(r"`([a-z-]+)`", head)) - {"/api/v1"}


def main() -> int:
    specs = load()
    errors: list[str] = []
    warnings: list[str] = []

    # --- commands -----------------------------------------------------------
    commands = declared_commands(specs["cli-commands"])
    for cap, text in specs.items():
        for m in CMD_RE.finditer(text):
            name = m.group(1)
            if name in CMD_IGNORE or name.startswith("--"):
                continue
            if name not in commands:
                errors.append(f"{cap}: `cyber {name}` is not in the cli-commands command tree")

    # --- routes -------------------------------------------------------------
    groups = declared_groups(specs["server-api"])
    for cap, text in specs.items():
        for m in ROUTE_RE.finditer(text):
            seg = m.group(1)
            group = ROUTE_TO_GROUP.get(seg)
            if group is None:
                errors.append(f"{cap}: route segment /api/v1/{seg} has no known route group")
            elif group not in groups:
                errors.append(f"{cap}: route group `{group}` (/api/v1/{seg}) not declared in server-api")
            if seg not in ROUTE_SINGLETONS and seg not in ROUTE_TO_GROUP:
                warnings.append(f"{cap}: /api/v1/{seg} is neither a plural collection nor a known singleton")

    # --- ID prefixes --------------------------------------------------------
    owners: dict[str, set[str]] = defaultdict(set)
    for cap, text in specs.items():
        for m in PREFIX_DECL_RE.finditer(text):
            owners[next(g for g in m.groups() if g)].add(cap)
    for prefix, caps in sorted(owners.items()):
        if len(caps) > 1:
            warnings.append(f"ID prefix `{prefix}` declared in several specs: {', '.join(sorted(caps))}")

    # --- duplicate requirement names ---------------------------------------
    names: dict[str, list[str]] = defaultdict(list)
    phases: dict[str, str] = {}
    for cap, text in specs.items():
        reqs = requirements(text)
        caps_phases = [p for _, p, _ in reqs if p]
        phases[cap] = min(caps_phases) if caps_phases else "P0"
        for name, phase, _ in reqs:
            names[name].append(cap)
            if not phase:
                errors.append(f"{cap}: requirement '{name}' has no phase tag")
    for name, caps in sorted(names.items()):
        if len(caps) > 1:
            warnings.append(f"requirement name '{name}' appears in: {', '.join(caps)}")

    # --- phase dependencies -------------------------------------------------
    cap_names = set(specs)
    registries = {"Command tree", "Resource command groups", "Route groups", "Top-level keys",
                  "Built-in tool set", "Capability-owned tool catalog", "Layering order",
                  "Ruleset layering", "Prompt promotion at Safe Boundaries", "Plugin manifest"}
    for cap, text in specs.items():
        for name, phase, body in requirements(text):
            if not phase or name in registries:
                continue
            for ref in set(CODE_RE.findall(body)):
                if ref in cap_names and ref != cap and phases[ref] > phase:
                    warnings.append(
                        f"{cap} ({phase}) '{name}' references `{ref}` whose earliest phase is {phases[ref]}")

    # --- stray values -------------------------------------------------------
    for cap, text in specs.items():
        for m in PORT_RE.finditer(text):
            if m.group(1) in STALE_PORTS:
                errors.append(f"{cap}: stale port {m.group(1)} used; the default port is 4747")
        if "--output-format" in text:
            errors.append(f"{cap}: uses --output-format; the only output flag is --format")
        if re.search(r"--cd\b", text):
            errors.append(f"{cap}: uses --cd; the only working-directory flag is --cwd")

    for w in warnings:
        print(f"warn:  {w}")
    for e in errors:
        print(f"error: {e}")
    print(f"{len(errors)} errors, {len(warnings)} warnings")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
