#!/usr/bin/env python3
"""Capture the README screenshots from the real `cyber` TUI.

The binary runs in 160×48 and 100×30 pseudo-terminals (pyte as the emulator) against a scripted,
local-only OpenAI-compatible model and a local stdio MCP test server, in a temporary
HOME/CYBER_HOME with a demo project. No external services are contacted. Each screen is written
as HTML and turned into a PNG with headless Chrome.

    python3 -m venv /tmp/cyber-shots && /tmp/cyber-shots/bin/pip install pyte pillow
    /tmp/cyber-shots/bin/python scripts/capture_tui_screens.py --binary target/debug/cyber

The Stats history is seeded fixture data (usage rows for earlier days in the temporary
database); the machine stats in the sidebar are the fixed MACHINE sample, served through
CYBER_MACHINE_FIXTURE so no screenshot shows the capturing machine.
"""
import argparse
import base64
import os
import random
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time
import json
import ipaddress
import urllib.request
import urllib.parse

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(REPO, "scripts", "screens"))
try:
    from term import Tui, html_to_png, to_html, stop_group  # noqa: E402
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
class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, new_url):
        raise RuntimeError("fixture HTTP redirects are refused")


HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())


def loopback_url(value):
    """Never send a fixture credential to an external address, even through a proxy."""
    url = urllib.parse.urlparse(value)
    if (url.scheme not in ("http", "https") or url.username or url.password
            or url.query or url.fragment or url.path not in ("", "/")
            or not url.port or not ipaddress.ip_address(url.hostname).is_loopback):
        raise ValueError("fixture URL must be an explicit loopback HTTP(S) listener")
    return value.rstrip("/")


GB = 1024 ** 3  # the sidebar shows binary gigabytes
# 12 cores at 23% CPU, 14.2 of 32 GB memory in use, 412 of 926 GB disk in use.
MACHINE = {"hostname_label": "macOS 15.4", "cores_logical": 12, "cores_physical": 12,
           "cpu_idle_percent": 77.0, "load_avg": [2.1, 1.8, 1.6],
           "memory_total": 32 * GB, "memory_available": int(17.8 * GB),
           "disk": {"mount": "/", "total": 926 * GB, "available": 514 * GB}, "sampled_at": 0}


class Env:
    def __init__(self, binary, out, chrome):
        self.binary, self.out, self.chrome = binary, out, chrome
        self.root = os.path.realpath(tempfile.mkdtemp(prefix="cs-", dir="/tmp"))
        self.home, self.cyber_home = f"{self.root}/home", f"{self.root}/cyber-home"
        self.project = f"{self.home}/work/cybercode-cli"
        self.server = self.provider = self.peer = None
        self.tuis = []

    def env(self):
        return {"HOME": self.home, "CYBER_HOME": self.cyber_home, "CYBER_MODELS_PATH": f"{self.root}/models.json",
                "CYBER_MACHINE_FIXTURE": f"{self.root}/machine.json",
                "PATH": os.environ["PATH"], "TERM": "xterm-256color", "COLORTERM": "truecolor",
                "LANG": "en_US.UTF-8", "TZ": "UTC"}

    def cyber(self, *args, timeout=60):
        return subprocess.run([self.binary, *args], env=self.env(), cwd=self.project,
                              capture_output=True, text=True, timeout=timeout)

    # -- fixtures -----------------------------------------------------------------
    def machine_fixture(self):
        with open(f"{self.root}/machine.json", "w") as f:
            json.dump(MACHINE, f)

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
        subprocess.run(["git", "init", "-q", "-b", "main", self.project], check=True, env=self.env())
        subprocess.run(git + ["add", "."], check=True, env=self.env())
        subprocess.run(git + ["commit", "-qm", "Initial import"], check=True, env=self.env())

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
                                          port_file], start_new_session=True, env=self.env())
        for _ in range(200):
            if os.path.exists(port_file) and open(port_file).read():
                self.provider_port = int(open(port_file).read())
                return self.provider_port
            time.sleep(0.05)
        raise RuntimeError("the scripted provider did not start")

    def api(self, method, path, body=None):
        """Use the actual owner API, with only this fixture's generated local credential."""
        with open(f"{self.cyber_home}/state/server.json") as f:
            registration = json.load(f)
        with open(f"{self.cyber_home}/state/password") as f:
            password = f.read().strip()
        token = base64.b64encode(f"cyber:{password}".encode()).decode()
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(loopback_url(registration["url"]) + "/api/v1" + path, data=data, method=method,
                                        headers={"content-type": "application/json", "authorization": f"Basic {token}",
                                                 "x-cyber-directory": self.project})
        with HTTP.open(request, timeout=20) as response:
            value = json.load(response)
        return value.get("data", value)

    def edit_config(self, edit):
        path = f"{self.cyber_home}/config/cyber.jsonc"
        with open(path) as f:
            config = json.load(f)
        edit(config)
        with open(path, "w") as f:
            json.dump(config, f, indent=2)

    def seed_directory(self):
        """Real local Sessions, a child, and a paired loopback peer on the public routes."""
        self.peer = Env(self.binary, self.out, self.chrome)
        self.peer.machine_fixture()
        self.peer.project_fixture()
        self.peer.config(self.provider_port)
        self.peer.edit_config(lambda cfg: cfg.update({"peers": {"machine": "studio", "listen": {"address": "127.0.0.1:0"}}}))
        self.peer.start_server()
        self.peer.api("POST", "/sessions", {"mode": "default", "title": "Review API contracts"})
        invitation = self.peer.api("POST", "/peers/grants", {"name": "demo"})
        fields = urllib.parse.parse_qs(urllib.parse.urlparse(invitation["code"]).query)
        remote = {"name": "studio", "url": loopback_url(invitation["listener"]["url"]),
                  "fingerprint": invitation["listener"]["fingerprint"], "token": fields["t"][0]}
        self.edit_config(lambda cfg: cfg.update({"peers": {"machine": "demo", "remotes": [remote]}}))
        current = self.api("POST", "/sessions", {"mode": "default", "title": "Refactor the auth module"})
        self.api("POST", "/sessions", {"mode": "default", "title": "Update README examples"})
        self.api("POST", "/sessions", {"mode": "default", "title": "Review API contracts"})
        child = self.api("POST", "/sessions", {"mode": "default", "title": "Check regression tests", "parent_id": current["id"]})
        self.sidebar_child = child["id"]
        self.api("POST", f"/sessions/{child['id']}/prompt", {"parts": [{"type": "text", "text": "GATED SIDEBAR verify regression tests"}]})
        wait("scripted child reaches its hold", lambda: "SIDEBAR" in self.held(), 20)
        return current["id"]

    def ctl(self, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(f"http://127.0.0.1:{self.provider_port}{path}", data=data,
                                     headers={"content-type": "application/json"})
        return json.loads(HTTP.open(req, timeout=10).read())

    def held(self):
        return self.ctl("/ctl/held")["held"]

    def start_server(self):
        registration = f"{self.cyber_home}/state/server.json"
        if os.path.exists(registration):
            os.remove(registration)
        self.server_log = open(f"{self.root}/serve.log", "a")
        self.server = subprocess.Popen([self.binary, "serve", "--register", "--port", "0"], env=self.env(),
                                       cwd=self.project, stdin=subprocess.DEVNULL,
                                       stdout=self.server_log, stderr=self.server_log, start_new_session=True)
        deadline = time.time() + 40
        while not os.path.exists(registration):
            if time.time() > deadline:
                raise RuntimeError(f"cyber serve did not register; see {self.root}/serve.log")
            time.sleep(0.05)
        time.sleep(0.3)

    @staticmethod
    def stop_process(process):
        if process is None or process.returncode is not None:
            return
        stop_group(process.pid)
        process.wait(10)

    def stop_server(self):
        self.stop_process(self.server)
        self.server = None
        if getattr(self, "server_log", None):
            self.server_log.close()


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
            columns = ("event_id,session_id,root_id,parent_id,workflow_run_id,agent,purpose,failed,"
                       "provider,model,started_ms,ended_ms,input_tokens,output_tokens,reasoning_tokens,"
                       "cache_read_tokens,cache_write_tokens,prompt_tokens,cost,directory")
            db.executemany(f"INSERT INTO usage_step ({columns}) VALUES (" + ",".join("?" * 20) + ")", rows)

    def launch(self, *args, colors=DARK, cols=COLS, rows=ROWS):
        tui = Tui([self.binary, *args], self.env(), self.project, cols, rows, background=colors[2])
        self.tuis.append(tui)
        tui.wait_for("ctrl+p", 40)
        tui.pump(1.0)
        return tui

    # -- output -------------------------------------------------------------------
    def shot(self, tui, name, colors=DARK, rows=None):
        path = os.path.join(self.out, name)
        page = to_html(tui.screen, rows, fg=colors[0], bg=colors[1])
        html_to_png(page, path, self.chrome, width=60 + tui.screen.columns * 8,
                    height=40 + (tui.screen.lines if rows is None else len(rows)) * 16)
        print("wrote", path, flush=True)
        return path

    def close(self):
        errors = []

        def cleanup(action):
            try:
                action()
            except Exception as error:
                errors.append(error)

        for tui in self.tuis:
            cleanup(tui.close)
        self.tuis.clear()
        cleanup(self.stop_server)
        if self.peer:
            cleanup(self.peer.close)
            self.peer = None
        cleanup(lambda: self.stop_process(self.provider))
        self.provider = None
        if errors:
            raise RuntimeError(f"capture cleanup failed; fixture logs remain at {self.root}: {errors}") from errors[0]
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


def command(tui, name, wait=1.2):
    """Run a slash command: enter on a fully typed name runs it."""
    tui.type(name, 0.8)
    tui.key("enter", wait)
    if name in "\n".join(tui.text().splitlines()[-6:]):
        raise TimeoutError(f"{name} did not run:\n{tui.text()}")


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
    env.machine_fixture()
    env.project_fixture()
    env.config(env.start_provider())
    env.start_server()
    env.stop_server()
    env.seed_usage_history()
    env.start_server()

    current = env.seed_directory()
    tui = env.launch("--resume", current)
    wait("machine stats", lambda: "% idle" in tui.text(), 20, tui=tui)
    tui.pump(1)
    env.shot(tui, "welcome.png")

    tui.type("show me the plan as markdown")
    tui.key("enter")
    tui.wait_for("Ready to apply", 30)
    tui.wait_for("Refactor the auth module", 20)
    tui.pump(1.5)
    tui.wait_for("Sessions & agents", 20)
    tui.wait_for("peer:studio", 25)
    listing = env.api("GET", "/messaging/sessions?from=" + current)
    if "SIDEBAR" not in env.held() or not any(row["session_id"] == env.sidebar_child and row["state"] == "running" for row in listing):
        raise RuntimeError("the screenshot child is not running at capture")
    env.shot(tui, "transcript-sidebar.png")
    env.shot(tui, "sessions-sidebar.png")

    narrow = env.launch("--resume", current, cols=100, rows=30)
    narrow.key("ctrl+x", 0.1)
    narrow.key("b", 0.5)
    narrow.wait_for("Sessions & agents", 20)
    narrow.wait_for("peer:studio", 25)
    env.shot(narrow, "sessions-sidebar-drawer.png")
    narrow.close()

    command(tui, "/rename")
    tui.wait_for("Rename session", 10)
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

    command(tui, "/theme")
    tui.type("synthwave", 1.5)
    env.shot(tui, "themes.png")
    tui.key("esc", 1.0)

    command(tui, "/files", 2.0)
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

    command(tui, "/settings", 2.0)
    tui.key("3", 2.0)
    env.shot(tui, "usage.png")
    tui.key("4", 2.5)
    env.shot(tui, "stats.png")
    tui.key("esc", 1.0)

    tui.close()
    env.tui_config("cyber-light", "light")
    light = env.launch("--resume", current, colors=LIGHT)
    wait("machine stats", lambda: "% idle" in light.text(), 20, tui=light)
    light.wait_for("peer:studio", 25)
    env.shot(light, "light.png", colors=LIGHT)
    light.close()
    env.tui_config("cyber", "dark")
    tui = env.launch("--resume", current)

    for name, tag, n in (("audit", "A", 12), ("scan", "S", 3), ("tests", "T", 2), ("docs", "D", 2)):
        with open(os.path.join(env.project, f"{name}.js"), "w") as f:
            f.write(fanout(name, tag, n))
        out = env.cyber("workflows", "run", f"{name}.js")
        if out.returncode != 0:
            raise RuntimeError(f"workflows run {name}: {out.stderr}")
        time.sleep(0.4)
    wait("one held agent per running run", lambda: {"A-0", "S-0", "T-0"}.issubset(env.held()), 40, tui=tui)
    for prefix in ("S-0", "A-0", "A-1"):
        wait(f"held agent {prefix}", lambda: prefix in env.held(), 20, tui=tui)
        env.ctl("/ctl/release", {"tag": prefix})
        tui.pump(0.8)
    command(tui, "/workflows", 3.0)
    env.shot(tui, "workflows-monitor.png")
    tui.key("esc", 1.0)
    env.ctl("/ctl/auto", {"on": True})
    def finished():
        runs = json.loads(env.cyber("workflows", "runs", "--format", "json").stdout or "[]")
        runs = runs.get("data", runs) if isinstance(runs, dict) else runs
        return runs and all(r.get("status") not in ("running", "queued") for r in runs)
    wait("runs finished", finished, 120, step=1.0, tui=tui)
    tui.close()



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
