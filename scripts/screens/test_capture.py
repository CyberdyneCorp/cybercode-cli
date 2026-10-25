"""Local process/HTTP safety regressions for the real TUI screenshot harness.

Run with the capture venv: python -m unittest discover -s scripts/screens -p test_capture.py
"""
import importlib.util
import os
from pathlib import Path
import signal
import sqlite3
import tempfile
import subprocess
import sys
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest.mock import Mock, patch

SOURCE = Path(__file__).resolve().parents[1] / "capture_tui_screens.py"
SPEC = importlib.util.spec_from_file_location("capture", SOURCE)
capture = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(capture)


class CaptureSafety(unittest.TestCase):
    def test_exited_unreaped_leader_cleans_surviving_child(self):
        child_code = "import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);print('READY',flush=True);time.sleep(60)"
        parent_code = ("import subprocess,sys; p=subprocess.Popen([sys.executable,'-c'," + repr(child_code)
                       + "],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL);p.stdout.readline();print(p.pid,flush=True)")
        leader = subprocess.Popen([sys.executable, "-c", parent_code], stdout=subprocess.PIPE,
                                  text=True, start_new_session=True)
        child = int(leader.stdout.read())  # EOF observes exit without reaping the leader.
        leader.stdout.close()
        try:
            self.assertIsNone(leader.returncode)
            capture.Env.stop_process(leader)
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                try:
                    os.killpg(leader.pid, 0)
                except (ProcessLookupError, PermissionError):
                    state = subprocess.run(["ps", "-o", "stat=", "-p", str(child)], capture_output=True, text=True)
                    if state.returncode or state.stdout.strip().startswith("Z"):
                        break
                time.sleep(0.02)
            else:
                self.fail("owned descendant group survived cleanup")
            with patch.object(capture.os, "killpg", side_effect=AssertionError("repeated signal")):
                capture.Env.stop_process(leader)
        finally:
            # Only the child of this test, still in its known group, can be signalled.
            try:
                if os.getpgid(child) == leader.pid:
                    os.kill(child, signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                pass
            if leader.returncode is None:
                capture.Env.stop_process(leader)

    def test_macos_zombie_only_group_permission_is_not_a_live_process(self):
        import term
        with patch.object(term.os, "killpg", side_effect=PermissionError), \
                patch.object(term.subprocess, "check_output", return_value="123 Z\n123 Z+\n"):
            term.stop_group(123)

    def test_exiting_group_is_observed_until_no_live_members_remain(self):
        import term
        with patch.object(term.os, "killpg", side_effect=PermissionError), \
                patch.object(term.subprocess, "check_output", side_effect=["123 E\n", "123 Z\n"]), \
                patch.object(term.time, "sleep"):
            term.stop_group(123)

    def test_cleanup_attempts_every_component_after_one_failure(self):
        fixture = capture.Env("/unused", "/unused", "/unused")
        order = []
        tui = Mock()
        tui.close.side_effect = lambda: (_ for _ in ()).throw(RuntimeError("TUI failure"))
        fixture.tuis = [tui]
        fixture.stop_server = lambda: order.append("server")
        fixture.peer = Mock()
        fixture.peer.close.side_effect = lambda: order.append("peer")
        fixture.stop_process = lambda process: order.append("provider")
        try:
            with self.assertRaisesRegex(RuntimeError, "TUI failure"):
                fixture.close()
            self.assertEqual(order, ["server", "peer", "provider"])
        finally:
            import shutil
            shutil.rmtree(fixture.root)

    def test_group_permission_denial_with_living_member_is_reported(self):
        import term
        with patch.object(term.os, "killpg", side_effect=PermissionError), \
                patch.object(term.subprocess, "check_output", return_value="123 S\n"), \
                self.assertRaises(PermissionError):
            term.stop_group(123)

    def test_usage_fixture_supports_added_ledger_columns(self):
        migration = (SOURCE.parents[1] / "crates/cyber-store/migrations/20261018000000_usage_ledger.sql").read_text()
        schema = "CREATE TABLE usage_step" + migration.split("CREATE TABLE usage_step", 1)[1].split("STRICT;", 1)[0] + "STRICT;"
        with tempfile.TemporaryDirectory() as root:
            path = str(Path(root) / "usage.sqlite")
            with sqlite3.connect(path) as db:
                db.executescript(schema)
                db.execute("ALTER TABLE usage_step ADD COLUMN origin TEXT NOT NULL DEFAULT 'user'")
                db.execute("ALTER TABLE usage_step ADD COLUMN mcp_client TEXT")
            fixture = Mock(project="/synthetic/project")
            fixture.database.return_value = path
            capture.Env.seed_usage_history(fixture, days=2)
            with sqlite3.connect(path) as db:
                self.assertGreater(db.execute("SELECT count(*) FROM usage_step").fetchone()[0], 0)
                self.assertEqual(db.execute("SELECT DISTINCT origin,mcp_client FROM usage_step").fetchall(), [("user", None)])

    def test_real_pty_master_closes_before_wait_and_repeated_close_is_safe(self):
        import term
        tui = term.Tui([sys.executable, "-c", "import time;print('ready',flush=True);time.sleep(60)"],
                       {"PATH": os.environ["PATH"], "TERM": "xterm-256color"}, "/tmp", 80, 24)
        tui.wait_for("ready", 5)
        tui.close()
        self.assertIsNone(tui.fd)
        tui.close()
        with self.assertRaises(ChildProcessError):
            os.waitpid(tui.pid, os.WNOHANG)

    def test_owned_group_is_stopped_before_leader_is_reaped(self):
        order = []
        leader = Mock(pid=123, returncode=None)
        leader.wait.side_effect = lambda timeout: order.append("reap")
        with patch.object(capture, "stop_group", side_effect=lambda pid: order.append("group stopped")):
            capture.Env.stop_process(leader)
        self.assertEqual(order, ["group stopped", "reap"])

    def test_already_reaped_handle_never_signals_a_reused_group(self):
        leader = subprocess.Popen([sys.executable, "-c", "pass"], start_new_session=True)
        leader.wait()
        with patch.object(capture.os, "killpg", side_effect=AssertionError("reaped signal")):
            capture.Env.stop_process(leader)

    def test_fixture_urls_require_numeric_loopback_listeners(self):
        self.assertEqual(capture.loopback_url("http://127.0.0.1:1234/"), "http://127.0.0.1:1234")
        self.assertEqual(capture.loopback_url("https://[::1]:4321"), "https://[::1]:4321")
        for url in ("https://example.com:443", "http://localhost:1234", "http://127.0.0.1:3/path",
                    "http://user:secret@127.0.0.1:3", "http://127.0.0.1", "file:///tmp/fixture"):
            with self.subTest(url=url), self.assertRaises(ValueError):
                capture.loopback_url(url)

    def test_fixture_http_ignores_proxy_and_refuses_redirects(self):
        requests = []

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                requests.append(self.path)
                if self.path == "/redirect":
                    self.send_response(302)
                    self.send_header("Location", "/must-not-follow")
                else:
                    self.send_response(200)
                self.send_header("Content-Length", "0")
                self.end_headers()

            def log_message(self, *args):
                pass

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        base = f"http://127.0.0.1:{server.server_port}"
        try:
            with patch.dict(os.environ, {"HTTP_PROXY": "http://127.0.0.1:1", "http_proxy": "http://127.0.0.1:1",
                                         "ALL_PROXY": "http://127.0.0.1:1", "NO_PROXY": "", "no_proxy": ""}):
                with capture.HTTP.open(base + "/direct", timeout=2) as response:
                    self.assertEqual(response.status, 200)
                with self.assertRaisesRegex(RuntimeError, "redirects are refused"):
                    capture.HTTP.open(base + "/redirect", timeout=2)
            self.assertEqual(requests, ["/direct", "/redirect"])
        finally:
            server.shutdown()
            server.server_close()
            thread.join(5)


if __name__ == "__main__":
    unittest.main()
