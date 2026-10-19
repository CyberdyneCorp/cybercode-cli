"""A fake browser MCP server for cyber-app tests: the tools of the pinned browser server,
backed by plain HTTP fetches instead of a real browser.

argv[1] is a log file: one JSON line per start (pid, TMPDIR, cwd), call and exit, so a
test can see which process served which Session and that it ended. A page's console
messages are its `data-console="..."` attributes; the screenshot is a fake PNG whose
bytes depend on the page body.
"""
import base64
import hashlib
import json
import os
import re
import signal
import sys
import urllib.error
import urllib.request

LOG = sys.argv[1]
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))
page = {"url": "", "body": "", "status": 0}


def log(event, **fields):
    with open(LOG, "a") as output:
        output.write(json.dumps(dict(fields, event=event, pid=os.getpid())) + "\n")


def ended(*_):
    log("exit")
    sys.exit(0)


signal.signal(signal.SIGTERM, ended)
log("start", tmpdir=os.environ.get("TMPDIR", ""), cwd=os.getcwd())

OBJECT = {"type": "object", "properties": {}}
URL = {"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}
ELEMENT = {"type": "object", "properties": {"element": {"type": "string"}, "ref": {"type": "string"}}}
TYPING = {"type": "object", "properties": {"element": {"type": "string"}, "ref": {"type": "string"},
                                           "text": {"type": "string"}}}
SIZE = {"type": "object", "properties": {"width": {"type": "integer"}, "height": {"type": "integer"}}}
READ = {"readOnlyHint": True, "openWorldHint": False}
ACT = {"readOnlyHint": False, "destructiveHint": False, "openWorldHint": False}
TOOLS = [
    ("browser_navigate", URL, ACT),
    ("browser_snapshot", OBJECT, READ),
    ("browser_click", ELEMENT, ACT),
    ("browser_type", TYPING, ACT),
    ("browser_resize", SIZE, ACT),
    ("browser_take_screenshot", OBJECT, READ),
    ("browser_console_messages", OBJECT, READ),
    ("browser_network_requests", OBJECT, READ),
    # Published by the pinned server, never offered: they bypass the network check or
    # read files.
    ("browser_evaluate", OBJECT, ACT),
    ("browser_file_upload", OBJECT, ACT),
    ("browser_tabs", OBJECT, ACT),
    ("browser_navigate_back", OBJECT, ACT),
]


def text(value):
    return {"content": [{"type": "text", "text": value}]}


def navigate(url):
    try:
        with OPENER.open(url, timeout=5) as response:
            page.update(url=url, body=response.read().decode(), status=response.status)
    except urllib.error.HTTPError as error:
        page.update(url=url, body=error.read().decode(), status=error.code)
    except Exception as error:
        return {"isError": True, **text("navigation failed: %s" % error)}
    title = re.search(r"<title>(.*?)</title>", page["body"])
    return text("Page URL: %s\nPage Title: %s" % (url, title.group(1) if title else ""))


def call(name, arguments):
    log("call", name=name, arguments=arguments)
    if name == "browser_navigate":
        return navigate(arguments.get("url", ""))
    if name == "browser_snapshot":
        return text(page["body"])
    if name == "browser_take_screenshot":
        png = b"\x89PNG\r\n\x1a\n" + hashlib.sha256(page["body"].encode()).digest()
        return {"content": [
            {"type": "text", "text": "Took a screenshot of %s" % page["url"]},
            {"type": "image", "mimeType": "image/png", "data": base64.b64encode(png).decode()},
        ]}
    if name == "browser_console_messages":
        lines = ["[LOG] loaded %s" % page["url"]]
        lines += ["[ERROR] %s" % message for message in re.findall(r'data-console="([^"]*)"', page["body"])]
        return text("\n".join(lines))
    if name == "browser_network_requests":
        return text("[GET] %s => [%d]" % (page["url"], page["status"]))
    return text("%s done" % name)


def answer(method, params):
    if method == "initialize":
        return {"protocolVersion": "2025-11-25", "capabilities": {"tools": {}},
                "serverInfo": {"name": "fake-browser", "version": "1"}}
    if method == "tools/list":
        return {"tools": [{"name": n, "description": n, "inputSchema": s, "annotations": a}
                          for n, s, a in TOOLS]}
    if method == "tools/call":
        return call(params.get("name"), params.get("arguments") or {})
    return None


for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request or "method" not in request:
        continue
    result = answer(request["method"], request.get("params") or {})
    if result is None:
        reply = {"error": {"code": -32601, "message": "unknown"}}
    else:
        reply = {"result": result}
    print(json.dumps(dict(reply, jsonrpc="2.0", id=request["id"])), flush=True)
ended()
