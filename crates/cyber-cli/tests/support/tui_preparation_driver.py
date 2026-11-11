"""Drive the production TUI in a PTY; withhold one committed confirmation reply.

Only the caller's private fixture registration is redirected. No production fault
hook is used. The Rust caller supplies the orphaned native preparation and checks
source bytes, immutable receipts, messages and inference before/after this driver.
"""
import fcntl
import http.client
import http.server
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import socketserver
import struct
import subprocess
import sys
import termios
import threading
import time
from urllib.parse import urlsplit


class Proxy(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True

    def __init__(self, upstream):
        super().__init__(("127.0.0.1", 0), Forward)
        self.upstream = upstream
        self.calls = []
        self.held = threading.Event()
        self.release = threading.Event()
        self.receipt = None
        self.ack_busy_refused = False


class Forward(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass  # Owner authorization headers must never reach test logs.

    def do_GET(self):
        self.forward()

    def do_POST(self):
        self.forward()

    def forward(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        call = {"method": self.command, "path": self.path, "done": False}
        self.server.calls.append(call)
        if self.path.endswith("/acknowledge") and not self.server.ack_busy_refused:
            self.server.ack_busy_refused = True
            failure = json.dumps({
                "_tag": "ConflictError", "message": "Snapshot storage is busy",
            }).encode()
            call.update(status=409, error=failure.decode())
            self.send_response(409)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(failure)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.close_connection = True
            self.wfile.write(failure)
            self.wfile.flush()
            call["done"] = True
            return
        upstream = http.client.HTTPConnection(self.server.upstream, timeout=45)
        try:
            upstream.request(self.command, self.path, body, dict(self.headers))
            response = upstream.getresponse()
            call["status"] = response.status
            failure = None
            if self.path.endswith(("/review", "/acknowledge")) and response.status != 200:
                failure = response.read(65536)
                call["error"] = failure.decode("utf-8", errors="replace")
            if self.command == "POST" and "/rewind/preparations/" in self.path and self.path.endswith("/confirm"):
                payload = json.loads(response.read())
                assert response.status == 200, payload
                self.server.receipt = payload["data"]
                assert self.server.receipt["request_id"] == json.loads(body)["request_id"]
                self.server.held.set()
                assert self.server.release.wait(45), "test did not release withheld reply"
                return  # The killed TUI never receives even response headers.
            self.send_response(response.status)
            for name, value in response.getheaders():
                if name.lower() not in ("connection", "transfer-encoding", "content-length"):
                    self.send_header(name, value)
            self.send_header("Connection", "close")
            self.end_headers()
            self.close_connection = True
            if failure is not None:
                self.wfile.write(failure)
                self.wfile.flush()
            while block := response.read1(65536):
                self.wfile.write(block)
                self.wfile.flush()
            call["done"] = True
        except (BrokenPipeError, ConnectionResetError):
            pass  # PTY owner termination also closes its event stream.
        finally:
            upstream.close()


class Terminal:
    def __init__(self, binary, session):
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 50, 160, 0, 0))
        environment = dict(os.environ, TERM="xterm-256color", NO_COLOR="1")
        environment.pop("CI", None)
        self.child = subprocess.Popen(
            [binary, "--resume", session], stdin=slave, stdout=slave, stderr=slave,
            env=environment, start_new_session=True,
        )
        os.close(slave)
        self.master = master
        self.output = bytearray()

    def pump(self, seconds=0.25):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if not select.select([self.master], [], [], max(0, min(0.05, deadline - time.monotonic())))[0]:
                continue
            try:
                block = os.read(self.master, 65536)
            except OSError:
                return
            if not block:
                return
            self.output.extend(block)
            # Bound diagnostics, including terminals that keep animating.
            del self.output[:-1024 * 1024]
            if b"\x1b[6n" in block:
                self.send(b"\x1b[1;1R")
            if b"\x1b[?u" in block:
                self.send(b"\x1b[?0u")
            if b"\x1b[c" in block:
                self.send(b"\x1b[?1;2c")

    def text(self):
        return re.sub(rb"\x1b(?:\[[0-?]*[ -/]*[@-~]|\].*?(?:\x07|\x1b\\))", b"", bytes(self.output), flags=re.DOTALL)

    def send(self, keys):
        os.write(self.master, keys)

    def wait(self, predicate, description):
        deadline = time.monotonic() + 30
        while not predicate():
            assert self.child.poll() is None, f"TUI exited during {description}: {self.text()[-3000:]!r}"
            assert time.monotonic() < deadline, f"TUI timed out during {description}: {self.text()[-3000:]!r}"
            self.pump()
        self.pump()  # Let the real runner apply the completed HTTP response.

    def stop(self, kill=False):
        if self.child.poll() is None:
            self.child.send_signal(signal.SIGKILL if kill else signal.SIGTERM)
        self.child.wait(timeout=10)
        self.pump(0.05)
        os.close(self.master)
        if kill:
            assert self.child.returncode == -signal.SIGKILL
        else:
            assert self.child.returncode == 0, bytes(self.output[-5000:])


def completed(proxy, suffix, success=True):
    return sum(
        call["done"] and call["path"].endswith(suffix)
        and (not success or call["status"] == 200)
        for call in proxy.calls
    )


def open_preparations(terminal, proxy):
    history = completed(proxy, "/preparations?include_cancelled=true")
    receipt = completed(proxy, "/receipt")
    terminal.wait(lambda: b"ownerdraft" in re.sub(rb"\s+", b"", terminal.text()), "saved draft rendering")
    terminal.send(b"\x1b")
    terminal.pump(0.1)
    terminal.send(b"\x1b")
    # C is ignored while rewind loads; retrying this read-only entry never
    # confirms source work. P must also work after journal inspection fails on
    # a genuinely unconstructed pre-origin namespace.
    for _ in range(6):
        terminal.pump(0.5)
        terminal.send(b"c")
        terminal.pump()
        if b"SNAPSHOTRECOVERY" in re.sub(rb"\s+", b"", terminal.text()):
            break
    terminal.wait(lambda: b"SNAPSHOTRECOVERY" in re.sub(rb"\s+", b"", terminal.text()), "recovery entry")
    for _ in range(6):
        terminal.send(b"p")
        terminal.pump(0.5)
        if completed(proxy, "/preparations?include_cancelled=true") > history:
            break
    terminal.wait(lambda: completed(proxy, "/preparations?include_cancelled=true") > history, "preparation history")
    terminal.send(b"\r")
    terminal.wait(lambda: completed(proxy, "/receipt") > receipt, "exact receipt discovery")


def positive_review(terminal, proxy):
    for attempt in range(20):
        terminal.wait(lambda: completed(proxy, "/review", success=False) > attempt, "pre-effect review response")
        reply = [call for call in proxy.calls if call["done"] and call["path"].endswith("/review")][-1]
        if reply["status"] == 200:
            return attempt + 1
        error = json.loads(reply["error"])
        busy = error.get("_tag") == "SessionBusyError" or error == {
            "_tag": "ConflictError", "message": "Snapshot storage is busy",
        }
        assert reply["status"] == 409 and busy, reply
        # Rewind inspection can still own the Session while its native worker
        # exits. Retry only this typed busy refusal, with bounded breathing room;
        # Enter requests fresh discovery/review, never cancellation replay.
        terminal.pump(0.5)
        terminal.send(b"\r")
    raise AssertionError(f"native pre-effect review refused: {reply}; terminal: {terminal.text()[-3000:]!r}")

def positive_acknowledgement(terminal, proxy, key):
    path = f"/requests/{key}/acknowledge"
    for _ in range(20):
        attempts = completed(proxy, path, success=False)
        terminal.send(b"a")
        terminal.wait(lambda: completed(proxy, path, success=False) > attempts, "explicit reconciliation response")
        reply = [call for call in proxy.calls if call["done"] and call["path"].endswith(path)][-1]
        if reply["status"] == 200:
            return
        error = json.loads(reply["error"])
        busy = error.get("_tag") == "SessionBusyError" or error == {
            "_tag": "ConflictError", "message": "Snapshot storage is busy",
        }
        assert reply["status"] == 409 and busy, reply
        # Look up the same durable receipt before another explicit metadata action.
        # Cancellation review/confirmation and source effects are never repeated.
        lookups = completed(proxy, f"/requests/{key}")
        terminal.pump(0.5)
        terminal.send(b"r")
        terminal.wait(lambda: completed(proxy, f"/requests/{key}") > lookups, "retained request lookup after busy refusal")
    raise AssertionError(f"native reconciliation stayed busy: {reply}")


def exercise(binary, session, activity, proxy):
    first = Terminal(binary, session)
    try:
        open_preparations(first, proxy)
        review_attempts = positive_review(first, proxy)
        first.send(b"\r")
        first.wait(proxy.held.is_set, "committed confirmation with reply withheld")
        receipt = proxy.receipt
        assert receipt["activity_id"] == activity
        first.stop(kill=True)
    finally:
        if first.child.poll() is None:
            first.stop(kill=True)
        proxy.release.set()
    second = Terminal(binary, session)
    try:
        open_preparations(second, proxy)
        second.send(b"\r")  # Outcome Enter must never replay cancellation.
        second.pump()
        key = receipt["request_id"]
        second.send(b"r")
        second.wait(lambda: completed(proxy, f"/requests/{key}") == 1, "original request lookup")
        assert not any(call["path"].endswith("/acknowledge") for call in proxy.calls), "receipt discovery acknowledged native metadata without an explicit action"
        positive_acknowledgement(second, proxy, key)
        second.send(b"\x1b")
        second.pump()
        second.stop()
    finally:
        if second.child.poll() is None:
            second.stop(kill=True)
    confirmations = [call for call in proxy.calls if call["path"].endswith("/confirm")]
    assert len(confirmations) == 1, "fresh terminal replayed cancellation"
    assert completed(proxy, f"/requests/{key}/acknowledge") == 1
    assert proxy.ack_busy_refused, "busy reconciliation path was not exercised"
    assert completed(proxy, "/review", success=False) == review_attempts, "fresh terminal requested a new cancellation review"
    return receipt


def main():
    binary, session, activity = sys.argv[1:]
    state = Path(os.environ["CYBER_HOME"]) / "state"
    path = state / "server.json"
    original = path.read_bytes()
    registration = json.loads(original)
    draft = "owner draft [Pasted text #1 +1 lines]"
    chips = [{"kind": "text", "number": 1, "payload": {"source": "text", "text": "owner attachment"}}]
    kv_path = state / "kv.json"
    kv = json.loads(kv_path.read_text()) if kv_path.exists() else {}
    kv.update({f"draft:{session}": draft, f"chips:{session}": chips})
    kv_path.write_text(json.dumps(kv))
    proxy = Proxy(urlsplit(registration["url"]).netloc)
    threading.Thread(target=proxy.serve_forever, daemon=True).start()
    registration["url"] = f"http://127.0.0.1:{proxy.server_port}"
    path.write_text(json.dumps(registration))
    try:
        receipt = exercise(binary, session, activity, proxy)
        saved = json.loads(kv_path.read_text())
        assert saved[f"draft:{session}"] == draft, "TUI recovery changed the draft"
        assert saved[f"chips:{session}"] == chips, "TUI recovery changed attachment payloads"
        print(json.dumps(receipt))
    finally:
        path.write_bytes(original)
        proxy.release.set()
        proxy.shutdown()
        proxy.server_close()


if __name__ == "__main__":
    main()
