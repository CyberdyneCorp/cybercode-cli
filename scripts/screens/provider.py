"""A scripted, local-only OpenAI-compatible model for the README screenshots.

argv: port-file. Writes the bound port to port-file and serves on 127.0.0.1 only; it never
reaches the internet. Fixed answers keep the captures deterministic:

  title requests            -> "Refactor the auth module"
  a prompt with "markdown"  -> the Markdown sample (bold numbered list, bullets, Python)
  a prompt with "HOLD-TURN" -> held until released, so a Turn stays in progress
  `GATED <tag>` (Workflow agents) -> held until released
  anything else             -> "Done."

Control routes: GET /ctl/held -> {"held": [tags]}; POST /ctl/release {"tag"|"prefix"|"all"}.
"""
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT_FILE = sys.argv[1]
LOCK = threading.Lock()
HELD = {}  # key -> (tag, event)
SEQ = [0]
AUTO = [False]

MARKDOWN = """Here is the plan for the auth refactor:

1. **Extract the token check** into `auth::verify`.
2. **Cache the JWKS keys** for five minutes.
3. **Add a regression test** for expired tokens.

Notes:

- the public API stays the same
- `cargo test -p auth` covers the new paths

```python
def verify(token: str, keys: dict) -> bool:
    # reject expired tokens before checking the signature
    header, claims = decode(token)
    if claims["exp"] < now():
        return False
    return keys[header["kid"]].verify(token)
```

Ready to apply when you are."""


def text_of(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(p.get("text", "") for p in content if isinstance(p, dict))
    return content or ""


def last_user(body):
    for message in reversed(body.get("messages", [])):
        if message.get("role") == "user":
            return text_of(message)
    return ""


def gate_tag(text):
    if "HOLD-TURN" in text:
        return "TURN"
    at = text.find("GATED ")
    return text[at + 6:].split()[0].strip(".,\"'") if at >= 0 else None


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def reply_json(self, value):
        data = json.dumps(value).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path.startswith("/ctl/held"):
            with LOCK:
                return self.reply_json({"held": [tag for tag, _ in HELD.values()]})
        self.reply_json({"data": [{"id": "main", "object": "model"}]})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        if self.path == "/ctl/auto":
            AUTO[0] = bool(body.get("on"))
            body = {"all": True}
        if self.path in ("/ctl/release", "/ctl/auto"):
            with LOCK:
                for tag, event in HELD.values():
                    if body.get("all") or tag == body.get("tag") or tag.startswith(body.get("prefix", "\0")):
                        event.set()
            return self.reply_json({"ok": True})
        self.chat(body)

    def chat(self, body):
        messages = body.get("messages", [])
        system = " ".join(text_of(m) for m in messages if m.get("role") == "system")
        user = last_user(body)
        if not body.get("tools") and "title" in system.lower():
            return self.stream("Refactor the auth module")
        if messages and messages[-1].get("role") == "tool":
            return self.stream("Done.")
        tag = gate_tag(user)
        if tag and not AUTO[0]:
            event = threading.Event()
            with LOCK:
                SEQ[0] += 1
                key = SEQ[0]
                HELD[key] = (tag, event)
            event.wait(600)
            with LOCK:
                HELD.pop(key, None)
        if tag:
            return self.stream(f"Result {tag}.")
        if "markdown" in user:
            return self.stream(MARKDOWN)
        return self.stream("Done.")

    def stream(self, text):
        chunks = [{"choices": [{"index": 0, "delta": {"content": text[i:i + 40]}, "finish_reason": None}]}
                  for i in range(0, len(text), 40)]
        chunks.append({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                       "usage": {"prompt_tokens": 86400, "completion_tokens": 1240}})
        data = ("".join(f"data: {json.dumps(c)}\n\n" for c in chunks) + "data: [DONE]\n\n").encode()
        time.sleep(1.6)  # a plausible streaming time for the Turn marker
        try:
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(data)
        except OSError:
            pass


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
server.daemon_threads = True
with open(PORT_FILE, "w") as f:
    f.write(str(server.server_address[1]))
server.serve_forever()
