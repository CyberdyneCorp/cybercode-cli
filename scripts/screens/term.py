"""Drive a program in a pseudo-terminal with pyte, and render pyte screens to HTML and PNG."""
import fcntl
import html
import os
import pty
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time

import pyte
from PIL import Image, ImageChops

KEYS = {
    "enter": "\r", "esc": "\x1b", "tab": "\t", "up": "\x1b[A", "down": "\x1b[B",
    "right": "\x1b[C", "left": "\x1b[D", "ctrl+p": "\x10", "ctrl+x": "\x18", "ctrl+u": "\x15",
}
NAMED = {
    "black": "000000", "red": "cd3131", "green": "0dbc79", "brown": "e5e510", "yellow": "e5e510",
    "blue": "2472c8", "magenta": "bc3fbc", "cyan": "11a8cd", "white": "e5e5e5",
    "brightblack": "666666", "brightred": "f14c4c", "brightgreen": "23d18b", "brightyellow": "f5f543",
    "brightblue": "3b8eea", "brightmagenta": "d670d6", "brightcyan": "29b8db", "brightwhite": "ffffff",
}
CELL_H = 16  # px; the HTML row height equals the line height, so rows touch with no gaps
SENTINEL = (255, 0, 255)


class Screen(pyte.Screen):
    def __init__(self, cols, rows, reply):
        super().__init__(cols, rows)
        self._reply = reply

    def write_process_input(self, data):
        self._reply(data)


class Tui:
    """A program in a pty at cols×rows; `background` answers the OSC 11 query (light or dark)."""

    def __init__(self, argv, env, cwd, cols=160, rows=48, background="0b0b/0f0f/1414"):
        self.background = background
        pid, fd = pty.fork()
        if pid == 0:
            os.chdir(cwd)
            os.execve(argv[0], argv, env)
        self.pid, self.fd = pid, fd
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.screen = Screen(cols, rows, self._send)
        self.stream = pyte.ByteStream(self.screen)

    def _send(self, data):
        os.write(self.fd, data.encode() if isinstance(data, str) else data)

    def pump(self, seconds=0.3):
        end = time.time() + seconds
        while (left := end - time.time()) > 0:
            ready, _, _ = select.select([self.fd], [], [], min(left, 0.03))
            if not ready:
                continue
            try:
                data = os.read(self.fd, 65536)
            except OSError:
                return
            if not data:
                return
            if b"\x1b[6n" in data:
                self._send(f"\x1b[{self.screen.cursor.y + 1};{self.screen.cursor.x + 1}R")
            if b"\x1b]11;?" in data:
                self._send(f"\x1b]11;rgb:{self.background}\x1b\\")
            self.stream.feed(data)

    def key(self, name, wait=0.6):
        self._send(KEYS.get(name, name))
        self.pump(wait)

    def type(self, text, wait=0.4):
        for ch in text:
            self._send(ch)
            self.pump(0.015)
        self.pump(wait)

    def text(self):
        return "\n".join(line.rstrip() for line in self.screen.display)

    def wait_for(self, needle, timeout=30.0):
        end = time.time() + timeout
        while time.time() < end:
            self.pump(0.2)
            if needle in self.text():
                return True
        raise TimeoutError(f"{needle!r} never appeared:\n{self.text()}")

    def close(self):
        try:
            os.kill(self.pid, signal.SIGTERM)
            for _ in range(50):
                if os.waitpid(self.pid, os.WNOHANG)[0]:
                    return
                self.pump(0.1)
            os.kill(self.pid, signal.SIGKILL)
        except (ProcessLookupError, ChildProcessError):
            pass


def _color(value, default):
    if value == "default":
        return default
    if value in NAMED:
        return "#" + NAMED[value]
    return "#" + value if len(value) == 6 else default


def _style(cell, fg_default, bg_default):
    fg, bg = _color(cell.fg, fg_default), _color(cell.bg, bg_default)
    if cell.reverse:
        fg, bg = bg, fg
    style = f"color:{fg};background:{bg};"
    if cell.bold:
        style += "font-weight:bold;"
    if cell.italics:
        style += "font-style:italic;"
    if cell.underscore:
        style += "text-decoration:underline;"
    return style


def to_html(screen, rows=None, fg="#d6e4f0", bg="#0b0f14"):
    """The screen (or a row range) as HTML: one fixed-height div per row, runs of equal style
    merged into inline-block spans, so cell backgrounds fill the row with no seams."""
    out = []
    for y in rows or range(screen.lines):
        line, spans, run, style = screen.buffer[y], [], "", None
        for x in range(screen.columns):
            cell = line[x]
            s = _style(cell, fg, bg)
            if s != style and run:
                spans.append(f'<span style="{style}">{html.escape(run)}</span>')
                run = ""
            style, run = s, run + (cell.data or " ")
        if run:
            spans.append(f'<span style="{style}">{html.escape(run)}</span>')
        out.append(f'<div class="r">{"".join(spans)}</div>')
    css = (f"body{{margin:0;padding:12px;background:rgb{SENTINEL}}}"
           f".t{{display:inline-block;background:{bg};font-family:Menlo,monospace;font-size:13px}}"
           f".r{{height:{CELL_H}px;line-height:{CELL_H}px;white-space:pre}}"
           f".r span{{display:inline-block;height:{CELL_H}px;line-height:{CELL_H}px;vertical-align:top}}")
    return f"<!doctype html><meta charset=utf-8><style>{css}</style><div class=t>{''.join(out)}</div>"


def html_to_png(page, path, chrome, width=1500, height=900):
    """Screenshot `page` with headless Chrome at 2x and crop to the terminal area."""
    with tempfile.TemporaryDirectory() as tmp:
        src, raw = os.path.join(tmp, "page.html"), os.path.join(tmp, "raw.png")
        with open(src, "w") as f:
            f.write(page)
        subprocess.run([chrome, "--headless=new", "--disable-gpu", "--hide-scrollbars",
                        "--force-device-scale-factor=2", f"--window-size={width},{height}",
                        f"--screenshot={raw}", "file://" + src],
                       check=True, capture_output=True, timeout=120)
        image = Image.open(raw).convert("RGB")
    box = ImageChops.difference(image, Image.new("RGB", image.size, SENTINEL)).getbbox()
    image = image.crop(box)
    image.save(path, optimize=True)
    return image
