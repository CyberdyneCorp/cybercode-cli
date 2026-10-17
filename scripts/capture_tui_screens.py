#!/usr/bin/env python3
"""Capture the README screenshots from the real `cyber` TUI.

The binary runs in a 160×48 pseudo-terminal (pyte as the emulator) against a scripted,
local-only OpenAI-compatible model and a local stdio MCP test server, in a temporary
HOME/CYBER_HOME with a demo project. Nothing reaches the network. Each screen is written
as HTML and turned into a PNG with headless Chrome.

    python3 -m venv /tmp/cyber-shots && /tmp/cyber-shots/bin/pip install pyte pillow
    /tmp/cyber-shots/bin/python scripts/capture_tui_screens.py --binary target/debug/cyber

The Stats history is seeded fixture data (usage rows for earlier days in the temporary
database); the machine stats in the sidebar are live readings of the capturing machine.
"""
import argparse
import os
import random
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
import json
import urllib.request

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(REPO, "scripts", "screens"))
try:
    from term import Tui, html_to_png, to_html  # noqa: E402
    from PIL import Image  # noqa: E402
except ImportError as missing:
    sys.exit(f"{missing}. Install the capture dependencies into a venv first:\n"
             "  python3 -m venv /tmp/cyber-shots && /tmp/cyber-shots/bin/pip install pyte pillow\n"
             "  /tmp/cyber-shots/bin/python scripts/capture_tui_screens.py")

CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
MCP = os.path.join(REPO, "crates", "cyber-app", "tests", "support", "mcp_stdio.py")
COLS, ROWS = 160, 48
DARK = ("#d6e4f0", "#0b0f14", "0b0b/0f0f/1414")
LIGHT = ("#1f2933", "#ffffff", "ffff/ffff/ffff")


class Env:
    def __init__(self, binary, out, chrome):
        self.binary, self.out, self.chrome = binary, out, chrome
        self.root = os.path.realpath(tempfile.mkdtemp(prefix="cs-", dir="/tmp"))
        self.home, self.cyber_home = f"{self.root}/home", f"{self.root}/cyber-home"
        self.project = f"{self.home}/work/cybercode-cli"
        self.server = self.provider = None

    def env(self):
        return {"HOME": self.home, "CYBER_HOME": self.cyber_home, "CYBER_MODELS_PATH": f"{self.root}/models.json",
                "PATH": os.environ["PATH"], "TERM": "xterm-256color", "COLORTERM": "truecolor",
                "LANG": "en_US.UTF-8", "TZ": "UTC"}

    def cyber(self, *args, timeout=60):
        return subprocess.run([self.binary, *args], env=self.env(), cwd=self.project,
                              capture_output=True, text=True, timeout=timeout)

    # -- fixtures -----------------------------------------------------------------
    def project_fixture(self):
        """A small copy of this repository (docs, specs list, top-level files) under git."""
        os.makedirs(self.project)
        for name in ("README.md", "ROADMAP.md", "Cargo.toml", "Cargo.lock", "clippy.toml", "justfile"):
            shutil.copy(os.path.join(REPO, name), self.project)
        for directory in ("docs", "eval", "scripts", "sdk/typescript/src", "openspec/specs/tui"):
            src = os.path.join(REPO, directory)
            shutil.copytree(src, os.path.join(self.project, directory),
                            ignore=shutil.ignore_patterns("*.png", "*.gif", "node_modules", "__pycache__", "generated"))
        crates = os.path.join(self.project, "crates")
        for crate in ("cyber-cli", "cyber-server", "cyber-tui", "cyber-workflow"):
            os.makedirs(os.path.join(crates, crate, "src"))
            shutil.copy(os.path.join(REPO, "crates", crate, "Cargo.toml"), os.path.join(crates, crate))
            shutil.copy(os.path.join(REPO, "crates", crate, "src", "lib.rs" if crate != "cyber-cli" else "main.rs"),
                        os.path.join(crates, crate, "src"))
        git = ["git", "-C", self.project, "-c", "user.name=demo", "-c", "user.email=demo@example.com"]
        subprocess.run(["git", "init", "-q", "-b", "main", self.project], check=True)
        subprocess.run(git + ["add", "."], check=True)
        subprocess.run(git + ["commit", "-qm", "Initial import"], check=True)

    def config(self, provider_port, theme="cyber", mode="dark"):
        cfg = f"{self.cyber_home}/config"
        os.makedirs(cfg, exist_ok=True)
        with open(f"{self.root}/models.json", "w") as f:
            f.write("{}")
        config = {
            "providers": {"demo": {"api": {"type": "openai-compatible", "url": f"http://127.0.0.1:{provider_port}/v1",
                                           "settings": {"auth": "none"}},
                                   "models": {"sonnet": {"name": "Demo Sonnet",
                                                       "limits": {"context": 200000, "output": 8192},
                                                       "cost": {"input": 3, "output": 15}}}}},
            "model": "demo/sonnet",
            "permissions": {"workflow.run": {"inline:*": "allow", "*": "allow"}},
            "workflows": {"max_concurrent": 4, "max_running_runs": 3},
            "mcp": {"docs": {"type": "local", "command": sys.executable, "args": ["-u", MCP, "full"]}},
        }
        with open(f"{cfg}/cyber.jsonc", "w") as f:
            json.dump(config, f, indent=2)
        self.tui_config(theme, mode)

    def tui_config(self, theme, mode):
        with open(f"{self.cyber_home}/config/tui.jsonc", "w") as f:
            json.dump({"theme": theme, "theme_mode": mode, "recap": "off"}, f)
        kv = f"{self.cyber_home}/state/kv.json"
        if os.path.exists(kv):
            os.remove(kv)

    # -- processes ----------------------------------------------------------------
    def start_provider(self):
        port_file = f"{self.root}/provider.port"
        self.provider = subprocess.Popen([sys.executable, "-I", os.path.join(REPO, "scripts", "screens", "provider.py"),
                                          port_file])
        for _ in range(200):
            if os.path.exists(port_file) and open(port_file).read():
                self.provider_port = int(open(port_file).read())
                return self.provider_port
            time.sleep(0.05)
        raise RuntimeError("the scripted provider did not start")

    def ctl(self, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(f"http://127.0.0.1:{self.provider_port}{path}", data=data,
                                     headers={"content-type": "application/json"})
        return json.loads(urllib.request.urlopen(req, timeout=10).read())

    def held(self):
        return self.ctl("/ctl/held")["held"]

    def start_server(self):
        registration = f"{self.cyber_home}/state/server.json"
        if os.path.exists(registration):
            os.remove(registration)
        self.server_log = open(f"{self.root}/serve.log", "a")
        self.server = subprocess.Popen([self.binary, "serve", "--register", "--port", "0"], env=self.env(),
                                       cwd=self.project, stdin=subprocess.DEVNULL,
                                       stdout=self.server_log, stderr=self.server_log)
        deadline = time.time() + 40
        while not os.path.exists(registration):
            if self.server.poll() is not None or time.time() > deadline:
                raise RuntimeError(f"cyber serve did not register; see {self.root}/serve.log")
            time.sleep(0.05)
        time.sleep(0.3)

    def stop_server(self):
        if self.server and self.server.poll() is None:
            self.server.terminate()
            self.server.wait(30)

    def database(self):
        for base, _, files in os.walk(self.cyber_home):
            for name in files:
                if name.endswith(".db") or name.endswith(".sqlite"):
                    path = os.path.join(base, name)
                    with sqlite3.connect(path) as db:
                        if db.execute("SELECT 1 FROM sqlite_master WHERE name='usage_step'").fetchone():
                            return path
        raise RuntimeError("no Cyber database with usage_step found")

    def seed_usage_history(self, days=150):
        """Fixture usage for earlier days so the Stats heatmap and streaks have something to show."""
        rng = random.Random(7)
        now_ms = int(time.time() * 1000)
        rows = []
        for day in range(days, 0, -1):
            weekday = time.gmtime((now_ms // 1000) - day * 86400).tm_wday
            if rng.random() < (0.55 if weekday >= 5 else 0.12) and day > 14:
                continue
            for session in range(rng.randint(1, 3)):
                root = f"ses_seed{day:03d}{session}"
                start = now_ms - day * 86_400_000 + rng.randint(8, 18) * 3_600_000
                for step in range(rng.randint(3, 14)):
                    start += rng.randint(20_000, 240_000)
                    tokens_in, tokens_out = rng.randint(6_000, 48_000), rng.randint(400, 6_000)
                    cache = rng.randint(0, 60_000)
                    cost = (tokens_in * 3 + tokens_out * 15 + cache * 0.3) / 1_000_000
                    rows.append((f"evt_seed_{day}_{session}_{step}", root, root, None, None, "build", "turn", 0,
                                 "demo", "sonnet", start, start + rng.randint(3_000, 40_000), tokens_in,
                                 tokens_out, 0, cache, cache // 20, tokens_in + cache + cache // 20, cost,
                                 self.project))
        with sqlite3.connect(self.database()) as db:
            db.executemany("INSERT INTO usage_step VALUES (" + ",".join("?" * 20) + ")", rows)

    def launch(self, *args, colors=DARK):
        tui = Tui([self.binary, *args], self.env(), self.project, COLS, ROWS, background=colors[2])
        tui.wait_for("ctrl+p", 40)
        tui.pump(1.0)
        return tui

    # -- output -------------------------------------------------------------------
    def shot(self, tui, name, colors=DARK, rows=None):
        path = os.path.join(self.out, name)
        page = to_html(tui.screen, rows, fg=colors[0], bg=colors[1])
        html_to_png(page, path, self.chrome, width=1340, height=40 + ROWS * 16 if rows is None else 40 + len(rows) * 16)
        print("wrote", path, flush=True)
        return path

    def close(self):
        self.stop_server()
        if self.provider:
            self.provider.terminate()
        shutil.rmtree(self.root, ignore_errors=True)


def wait(what, check, timeout=40, step=0.2, tui=None):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if tui:
            tui.pump(step)
        else:
            time.sleep(step)
        if check():
            return
    raise TimeoutError(what)


def fanout(name, tag, n):
    return (f"export const meta = {{ name: '{name}', description: 'Fan out {n} agents', "
            f"phases: [{{ title: 'Fan out' }}, {{ title: 'Collect' }}] }};\n"
            f"export default async () => {{\n  phase('Fan out');\n"
            f"  const r = await parallel(Array.from({{ length: {n} }}, (_, i) => () => "
            f"agent(`GATED {tag}-${{i}}`, {{ label: `{tag.lower()}${{i}}`, isolation: 'none' }})));\n"
            f"  phase('Collect');\n  return r.length;\n}};\n")


def gif(env, tui, frames=24, name="thinking.gif"):
    """Consecutive pty captures of the progress line and the composer, assembled with PIL."""
    rows = list(range(ROWS - 9, ROWS))
    images = []
    for i in range(frames):
        tui.pump(0.09)
        images.append(html_to_png(to_html(tui.screen, rows), os.path.join(env.root, f"f{i}.png"), env.chrome,
                                  width=1340, height=40 + len(rows) * 16))
    w, h = images[0].size
    images = [im.resize((w // 2, h // 2), Image.LANCZOS).convert("P", palette=Image.ADAPTIVE, colors=96) for im in images]
    path = os.path.join(env.out, name)
    images[0].save(path, save_all=True, append_images=images[1:], duration=84, loop=0, optimize=True)
    print("wrote", path, f"({os.path.getsize(path) // 1024} KiB)", flush=True)


def capture(env):
    env.project_fixture()
    env.config(env.start_provider())
    env.start_server()
    env.stop_server()
    env.seed_usage_history()
    env.start_server()

    tui = env.launch()
    wait("machine stats", lambda: "% idle" in tui.text(), 20, tui=tui)
    tui.pump(1)
    env.shot(tui, "welcome.png")

    tui.type("show me the plan as markdown")
    tui.key("enter")
    tui.wait_for("Ready to apply", 30)
    tui.wait_for("Refactor the auth module", 20)
    tui.pump(1.5)
    env.shot(tui, "transcript-sidebar.png")

    tui.type("/rename")
    tui.key("enter", 1.2)
    env.shot(tui, "rename.png")
    tui.key("esc", 0.8)

    tui.type("HOLD-TURN keep thinking while I capture")
    tui.key("enter")
    wait("held turn", lambda: "TURN" in env.held(), 20, tui=tui)
    tui.pump(2.5)
    env.shot(tui, "thinking.png")
    gif(env, tui)
    env.ctl("/ctl/release", {"all": True})
    tui.wait_for("Result TURN", 20)
    tui.pump(1)

    tui.type("/theme")
    tui.key("enter", 1.2)
    tui.type("synthwave", 1.5)
    env.shot(tui, "themes.png")
    tui.key("esc", 1.0)

    tui.type("/files")
    tui.key("enter", 2.0)
    tui.wait_for("docs/", 20)
    lines = tui.text().splitlines()
    first = next(i for i, l in enumerate(lines) if "crates/" in l)
    docs = next(i for i, l in enumerate(lines) if "docs/" in l)
    for _ in range(docs - first):
        tui.key("down", 0.2)
    tui.key("right", 1.5)
    lines = tui.text().splitlines()
    readme = next(i for i, l in enumerate(lines) if i > docs and "README.md" in l)
    for _ in range(readme - docs):
        tui.key("down", 0.2)
    tui.key("enter", 2.5)
    env.shot(tui, "explorer.png")
    tui.key("esc", 0.8)
    tui.key("esc", 0.8)

    tui.type("/settings")
    tui.key("enter", 2.0)
    tui.key("3", 2.0)
    env.shot(tui, "usage.png")
    tui.key("4", 2.5)
    env.shot(tui, "stats.png")
    tui.key("esc", 1.0)

    for name, tag, n in (("audit", "A", 12), ("scan", "S", 3), ("tests", "T", 2), ("docs", "D", 2)):
        with open(os.path.join(env.project, f"{name}.js"), "w") as f:
            f.write(fanout(name, tag, n))
        out = env.cyber("workflows", "run", f"{name}.js")
        if out.returncode != 0:
            raise RuntimeError(f"workflows run {name}: {out.stderr}")
        time.sleep(0.4)
    wait("agents at the model", lambda: len(env.held()) >= 4, 40, tui=tui)
    for prefix in ("S-0", "A-0", "A-1"):
        env.ctl("/ctl/release", {"tag": prefix})
        tui.pump(0.8)
    tui.type("/workflows")
    tui.key("enter", 3.0)
    env.shot(tui, "workflows-monitor.png")
    tui.key("esc", 1.0)
    env.ctl("/ctl/auto", {"on": True})
    def finished():
        runs = json.loads(env.cyber("workflows", "runs", "--format", "json").stdout or "[]")
        runs = runs.get("data", runs) if isinstance(runs, dict) else runs
        return runs and all(r.get("status") not in ("running", "queued") for r in runs)
    wait("runs finished", finished, 120, step=1.0, tui=tui)
    tui.close()

    env.tui_config("cyber-light", "light")
    session = json.loads(env.cyber("sessions", "list", "--format", "json").stdout)
    first = next(s for s in session if s.get("title") == "Refactor the auth module")
    light = env.launch("--resume", first["id"], colors=LIGHT)
    wait("machine stats", lambda: "% idle" in light.text(), 20, tui=light)
    light.pump(1)
    env.shot(light, "light.png", colors=LIGHT)
    light.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--binary", default=os.path.join(REPO, "target", "debug", "cyber"))
    parser.add_argument("--out", default=os.path.join(REPO, "docs", "screenshots"))
    parser.add_argument("--chrome", default=os.environ.get("CYBER_SHOTS_CHROME", CHROME))
    args = parser.parse_args()
    if not os.path.exists(args.chrome):
        sys.exit(f"Chrome not found at {args.chrome}; pass --chrome or set CYBER_SHOTS_CHROME. Skipping the capture.")
    if not os.access(args.binary, os.X_OK):
        sys.exit(f"{args.binary} is missing; build it first with `cargo build -p cyber-cli`.")
    os.makedirs(args.out, exist_ok=True)
    env = Env(os.path.abspath(args.binary), args.out, args.chrome)
    try:
        capture(env)
    finally:
        env.close()


if __name__ == "__main__":
    main()
