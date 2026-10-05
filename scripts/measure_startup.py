#!/usr/bin/env python3
"""Measure a real release CLI process through its first complete PTY frame.

Uses isolated application data and a configured offline model (no inference).
Responds to terminal capability negotiation, then observes the cursor-show command
emitted at the end of ratatui's draw. No private timing hook or test renderer.
"""
import argparse
import fcntl
import json
import math
import os
import platform
import pty
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time
from pathlib import Path


def environment(root):
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(('CYBER_', 'XDG_', 'OPENAI_', 'ANTHROPIC_'))}
    env.update(CYBER_HOME=str(root), CYBER_OFFLINE='1', TERM='xterm-256color')
    config = root / 'config'
    config.mkdir()
    (config / 'cyber.json').write_text(json.dumps({
        'model': 'probe/offline',
        'providers': {'probe': {'api': {'type': 'openai-compatible',
            'url': 'http://127.0.0.1:1/v1', 'settings': {'auth': 'none'}},
            'models': {'offline': {}}}},
    }))
    return env


def respond(fd, buffer):
    # Crossterm checks both Kitty protocol and primary device attributes.
    for query, answer in [(b'\x1b[?u', b'\x1b[?0u'),
                          (b'\x1b[c', b'\x1b[?1;2c'),
                          (b'\x1b[6n', b'\x1b[1;1R')]:
        if query in buffer:
            os.write(fd, answer)
            buffer = buffer.replace(query, b'')
    return buffer[-16:]


def first_frame(binary, env, directory, embedded):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 32, 120, 0, 0))
    args = [str(binary), '--cwd', str(directory)]
    if embedded:
        args.append('--embedded')
    start = time.perf_counter_ns()
    process = subprocess.Popen(args, stdin=slave, stdout=slave, stderr=slave,
                               env=env, start_new_session=True)
    os.close(slave)
    try:
        elapsed = await_frame(master, process, start)
        os.write(master, b'\x04')  # Ctrl-D: clean TUI teardown.
        process.wait(timeout=5)
        if process.returncode != 0:
            raise RuntimeError(f'TUI exit status {process.returncode}')
        return elapsed
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=5)
        os.close(master)


def await_frame(master, process, start):
    transcript, pending = b'', b''
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if not select.select([master], [], [], 0.02)[0]:
            if process.poll() is not None:
                break
            continue
        chunk = os.read(master, 65536)
        transcript += chunk
        pending = respond(master, pending + chunk)
        if b'Enter send' in transcript and b'\x1b[?25h' in transcript:
            return (time.perf_counter_ns() - start) / 1e6
    raise RuntimeError(f'No complete first frame: {transcript[-2000:]!r}')


def summary(samples):
    ordered = sorted(samples)
    return {**{f'p{p}_ms': ordered[math.ceil(len(ordered) * p / 100) - 1]
               for p in (50, 95, 99)}, 'min_ms': min(samples), 'max_ms': max(samples)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path('target/release/cyber'))
    parser.add_argument('--runs', type=int, default=20)
    parser.add_argument('--machine', required=True)
    args = parser.parse_args()
    if args.runs < 1:
        parser.error('--runs must be positive')
    binary = args.binary.resolve(strict=True)
    results = {}
    for mode in ('embedded', 'service-cold'):
        samples = []
        for _ in range(args.runs):
            with tempfile.TemporaryDirectory(prefix='cyber-startup-') as temp:
                root = Path(temp)
                env = environment(root)
                repo = root / 'repo'
                repo.mkdir()
                try:
                    samples.append(first_frame(binary, env, repo, mode == 'embedded'))
                finally:
                    if mode == 'service-cold':
                        subprocess.run([str(binary), 'service', 'stop'], env=env,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                       timeout=10, check=False)
        results[mode] = {'samples_ms': samples, **summary(samples)}
    print(json.dumps({'machine': args.machine, 'platform': platform.platform(),
                      'binary': str(args.binary), 'runs': args.runs,
                      'terminal': 'xterm-256color PTY 120x32',
                      'cache': 'OS file cache retained; fresh app data and processes per sample',
                      'results': results}, indent=2))


if __name__ == '__main__':
    main()
