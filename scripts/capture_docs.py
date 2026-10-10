#!/usr/bin/env python3
"""Render genuine CLI/PTY output for docs; requires Pillow and pyte.

Run with an isolated offline model and embedded server. No prompts are submitted.
"""
import argparse
import codecs
import fcntl
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time

import pyte
from PIL import Image, ImageDraw, ImageFont
from measure_startup import environment, respond

COLS, ROWS = 112, 30
PALETTE = dict(black='#151a23', red='#ef7276', green='#94d39c', brown='#e7be78',
               blue='#80aaff', magenta='#cf9cf1', cyan='#80d6db', white='#d8dee9',
               brightblack='#697588', brightred='#ff9295', brightgreen='#b3edba',
               brightbrown='#ffe1a6', brightblue='#abc7ff', brightmagenta='#e6bbff',
               brightcyan='#b0f3f5', brightwhite='#ffffff')


def color(value, default):
    if value == 'default':
        return default
    return PALETTE.get(value, '#' + value)


def render(screen, path, title, font_path):
    font = ImageFont.truetype(str(font_path), 18)
    width = round(font.getlength('M'))
    height, pad, heading = 26, 24, 50
    image = Image.new('RGB', (COLS * width + 2 * pad, screen.lines * height + 2 * pad + heading), '#151a23')
    draw = ImageDraw.Draw(image)
    draw.text((pad, 14), title, font=font, fill='#80d6db')
    for row in range(screen.lines):
        for col in range(COLS):
            char = screen.buffer[row][col]
            fg, bg = color(char.fg, '#d8dee9'), color(char.bg, '#151a23')
            if char.reverse:
                fg, bg = bg, fg
            x, y = pad + col * width, pad + heading + row * height
            draw.rectangle((x, y, x + width - 1, y + height - 1), fill=bg)
            draw.text((x, y), char.data, font=font, fill=fg)
    image.save(path)


def capture_tui(binary, env, repo):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', ROWS, COLS, 0, 0))
    process = subprocess.Popen([str(binary), '--embedded', '--cwd', str(repo)],
                               stdin=slave, stdout=slave, stderr=slave, env=env, start_new_session=True)
    os.close(slave)
    screen, pending, raw = pyte.Screen(COLS, ROWS), b'', b''
    stream, decoder = pyte.Stream(screen), codecs.getincrementaldecoder('utf-8')('replace')
    try:
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if not select.select([master], [], [], 0.05)[0]:
                continue
            chunk = os.read(master, 65536)
            raw += chunk
            pending = respond(master, pending + chunk)
            stream.feed(decoder.decode(chunk))
            if b'Enter send' in raw and b'\x1b[?25h' in raw:
                # Let local readiness updates settle, without making an inference call.
                settle = time.monotonic() + 1
                while time.monotonic() < settle:
                    if select.select([master], [], [], 0.05)[0]:
                        chunk = os.read(master, 65536)
                        pending = respond(master, pending + chunk)
                        stream.feed(decoder.decode(chunk))
                return screen
        raise RuntimeError('No complete TUI frame')
    finally:
        os.write(master, b'\x04')
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=5)
        os.close(master)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path('target/debug/cyber'))
    parser.add_argument('--font', type=Path, default=Path('/System/Library/Fonts/Monaco.ttf'))
    parser.add_argument('--output', type=Path, default=Path('docs/screenshots'))
    args = parser.parse_args()
    binary = args.binary.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='cyber-docs-', dir='/tmp') as temporary:
        root = Path(temporary)
        home, repo = root / 'home', root / 'demo-repo'
        home.mkdir()
        repo.mkdir()
        env = environment(home)
        # Isolated HOME prevents consulting personal credentials or configuration.
        env['HOME'] = str(home)
        render(capture_tui(binary, env, repo), args.output / 'tui.png', 'cyber --embedded | offline startup', args.font)
        output = subprocess.run([str(binary), '--help'], env=env, cwd=repo,
                                check=True, capture_output=True, text=True).stdout
        # The commands section fits one readable terminal image; output is verbatim.
        text = output.split('\nOptions:')[0]
        lines = text.splitlines()
        screen = pyte.Screen(COLS, len(lines) + 1)
        pyte.Stream(screen).feed(text.replace('\n', '\r\n'))
        render(screen, args.output / 'cli.png', 'cyber --help | command reference', args.font)
    print('Captured TUI and CLI without model inference.')


if __name__ == '__main__':
    main()
