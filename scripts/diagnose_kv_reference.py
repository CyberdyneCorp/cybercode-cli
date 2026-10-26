#!/usr/bin/env python3
"""Observe unchanged KV reference controls; do not repair or relax historical inputs.

The repeated native test and the injected ps-error counterexample are separate evidence.
Observing the latter does not establish why an unrelated CI attempt failed.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import signal
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REFERENCE = ROOT / "eval/solutions/fix-shell-kv-store/kv.sh"
HIDDEN = ROOT / "eval/graders/fix-shell-kv-store/hidden_test_kv.py"
CASE = "hidden_test_kv.LockTest.test_concurrent_incr_with_stale_lock"
MAX_LOG_BYTES = 1024 * 1024


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2) + "\n")


def log_text(value: str | bytes | None, temporary: Path) -> str:
    raw = value.encode() if isinstance(value, str) else value or b""
    text = raw[:MAX_LOG_BYTES].decode("utf-8", errors="replace")
    for path, label in [
        (str(ROOT), "<repository>"),
        (str(temporary), "<control-temp>"),
    ]:
        text = text.replace(path, label)
    if len(raw) > MAX_LOG_BYTES:
        text += "\n<diagnostic log truncated at 1 MiB>\n"
    return text


def environment_metadata() -> dict:
    bash = subprocess.run(
        ["bash", "--version"], capture_output=True, text=True, timeout=10
    )
    metadata = {
        "python": platform.python_version(),
        "platform": platform.system(),
        "kernel": platform.release(),
        "bash": bash.stdout.splitlines()[0] if bash.stdout else "unavailable",
        "source_sha256": {
            "reference": digest(REFERENCE),
            "hidden_test": digest(HIDDEN),
        },
    }
    for label, path in [
        ("kernel_pid_max", "/proc/sys/kernel/pid_max"),
        ("cgroup_pids_max", "/sys/fs/cgroup/pids.max"),
        ("legacy_cgroup_pids_max", "/sys/fs/cgroup/pids/pids.max"),
    ]:
        if Path(path).is_file():
            metadata[label] = Path(path).read_text()[:1024].strip()
    limits = Path("/proc/self/limits")
    if limits.is_file():
        metadata["process_limits"] = [
            line
            for line in limits.read_text().splitlines()
            if line.startswith("Max processes")
        ]
    return metadata


def unchanged_test(
    output: Path, temporary: Path, attempts: int, trace: bool
) -> tuple[list[dict], int]:
    observations = []
    deadline = time.monotonic() + 600
    for number in range(1, attempts + 1):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return observations, 2
        with tempfile.TemporaryDirectory(prefix="kv-native-", dir=temporary) as name:
            workspace = Path(name)
            shutil.copy2(REFERENCE, workspace / "kv.sh")
            shutil.copy2(HIDDEN, workspace / "hidden_test_kv.py")
            started = time.monotonic()
            row = {"attempt": number, "kind": "unchanged_native_test"}
            command = [sys.executable, "-m", "unittest", "-v", CASE]
            trace_path = workspace / "syscalls.log"
            if trace:
                command = [
                    "strace",
                    "-f",
                    "-ttt",
                    "-s",
                    "256",
                    "-e",
                    "trace=%file,%process",
                    "-o",
                    str(trace_path),
                    *command,
                ]
            process = subprocess.Popen(
                command,
                cwd=workspace,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                start_new_session=True,
                env=dict(
                    os.environ, TMPDIR=str(temporary), PYTHONDONTWRITEBYTECODE="1"
                ),
            )
            try:
                stdout, stderr = process.communicate(timeout=min(120, remaining))
                row["exit"] = process.returncode
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                stdout, stderr = process.communicate()
                row.update(exit=None, timeout=True)
            row["seconds"] = round(time.monotonic() - started, 4)
            row["copied_sources_unchanged"] = digest(workspace / "kv.sh") == digest(
                REFERENCE
            ) and digest(workspace / "hidden_test_kv.py") == digest(HIDDEN)
            (output / f"attempt-{number:02d}.stdout.txt").write_text(
                log_text(stdout, temporary)
            )
            (output / f"attempt-{number:02d}.stderr.txt").write_text(
                log_text(stderr, temporary)
            )
            observations.append(row)
            write_json(output / "native-observations.json", observations)
            print(f"Unchanged native attempt {number}: exit {row['exit']}", flush=True)
            if row["exit"] != 0 or not row["copied_sources_unchanged"]:
                if trace_path.exists():
                    written = 0
                    with (output / f"attempt-{number:02d}.lock-trace.txt").open(
                        "w"
                    ) as saved:
                        for line in trace_path.open(errors="replace"):
                            if (
                                ".lock" in line
                                or "execve(" in line
                                or "exited with" in line
                            ):
                                line = log_text(line, temporary)
                                written += len(line.encode())
                                if written > 8 * MAX_LOG_BYTES:
                                    saved.write("<lock trace truncated at 8 MiB>\n")
                                    break
                                saved.write(line)
                return observations, 1
    return observations, 0


def injected_probe(output: Path, temporary: Path) -> dict:
    """One non-stress counterexample: ps exits 2 while the recorded holder is alive."""
    with tempfile.TemporaryDirectory(prefix="kv-ps-error-", dir=temporary) as name:
        workspace = Path(name)
        source = workspace / "kv.sh"
        shutil.copy2(REFERENCE, source)
        store = workspace / "store"
        lock = store / ".lock"
        lock.mkdir(parents=True)
        holder = os.getpid()
        (lock / "pid").write_text(f"{holder}\n")
        real_ps = shutil.which("ps")
        if real_ps is None:
            return {"kind": "injected_ps_status_2", "status": "ps_unavailable"}
        before = subprocess.run(
            [real_ps, "-p", str(holder)], capture_output=True, timeout=10
        )
        binary = workspace / "bin"
        binary.mkdir()
        shim = binary / "ps"
        shim.write_text(
            "#!/bin/sh\n"
            f'if [ "$#" -eq 2 ] && [ "$1" = -p ] && [ "$2" = {holder} ]; then exit 2; fi\n'
            f'exec {shlex.quote(real_ps)} "$@"\n'
        )
        shim.chmod(0o700)
        result = subprocess.run(
            ["bash", str(source), "-d", str(store), "incr", "counter", "2"],
            capture_output=True,
            timeout=10,
            env=dict(
                os.environ,
                PATH=str(binary) + os.pathsep + os.environ.get("PATH", ""),
                KV_LOCK_TIMEOUT="1",
            ),
        )
        after = subprocess.run(
            [real_ps, "-p", str(holder)], capture_output=True, timeout=10
        )
        counter = store / "counter"
        observed = (
            before.returncode == after.returncode == 0
            and result.returncode == 0
            and counter.is_file()
            and counter.read_bytes() == b"2"
        )
        (output / "injected-probe.stdout.txt").write_text(
            log_text(result.stdout, temporary)
        )
        (output / "injected-probe.stderr.txt").write_text(
            log_text(result.stderr, temporary)
        )
        return {
            "kind": "injected_ps_status_2",
            "holder_alive_before_and_after": before.returncode == after.returncode == 0,
            "command_exit": result.returncode,
            "unsafe_live_lock_reclaimed": observed,
            "interpretation": "An injected probe-error counterexample; not proof of the original CI failure's cause.",
        }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--attempts", type=int, default=30)
    parser.add_argument("--trace", action="store_true")
    args = parser.parse_args()
    if not 1 <= args.attempts <= 30:
        parser.error("attempts must be between 1 and 30")
    args.output.mkdir(parents=True, exist_ok=False)
    temporary = Path(os.environ.get("TMPDIR", tempfile.gettempdir())).resolve()
    temporary.mkdir(parents=True, exist_ok=True)
    metadata = environment_metadata()
    write_json(args.output / "environment.json", metadata)
    observations, status = unchanged_test(
        args.output, temporary, args.attempts, args.trace
    )
    probe = injected_probe(args.output, temporary)
    write_json(args.output / "injected-probe.json", probe)
    unchanged = metadata["source_sha256"] == {
        "reference": digest(REFERENCE),
        "hidden_test": digest(HIDDEN),
    }
    write_json(
        args.output / "summary.json",
        {
            "native_attempts": len(observations),
            "native_status": (
                "no_failure_observed"
                if status == 0
                else "failure_observed" if status == 1 else "diagnostic_time_limit"
            ),
            "original_sources_unchanged": unchanged,
            "injected_probe_is_separate_from_native_results": True,
            "injected_ps_error_counterexample_observed": probe.get(
                "unsafe_live_lock_reclaimed", False
            ),
        },
    )
    return status if unchanged else 2


if __name__ == "__main__":
    raise SystemExit(main())
