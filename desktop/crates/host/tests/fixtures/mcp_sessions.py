"""Local-only stateful MCP fixture for retained connection regression tests."""
import json
import os
import queue
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit


def result(request, state):
    if request["method"] == "initialize":
        return {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}},
                "serverInfo": {"name": "stateful-fixture", "version": "1"}}
    if request["method"] == "tools/list":
        return {"tools": [{"name": "counter", "inputSchema": {"type": "object"}}]}
    arguments = request.get("params", {}).get("arguments", {})
    if arguments.get("delay"):
        time.sleep(30)
    state["counter"] += 1
    return {"content": [{"type": "text", "text": str(state["counter"])}],
            "counter": state["counter"], "label": os.environ.get("SESSION_LABEL", ""),
            "isError": arguments.get("fail", False)}


def response(request, state):
    arguments = request.get("params", {}).get("arguments", {})
    frame = {"jsonrpc": "2.0", "id": request["id"]}
    if arguments.get("rpc_error"):
        frame["error"] = {"code": -32602, "message": "fixture invalid params"}
    elif arguments.get("malformed"):
        frame["unexpected"] = "neither result nor error"
    else:
        frame["result"] = result(request, state)
    return frame


if len(sys.argv) == 1:
    state = {"counter": 0}
    for line in sys.stdin:
        request = json.loads(line)
        if "id" in request:
            if request.get("params", {}).get("arguments", {}).get("disconnect"):
                break
            print(json.dumps(response(request, state)), flush=True)
else:
    sessions = {}
    lock = threading.Lock()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def reply(self, code, value=None, sid=None):
            body = b"" if value is None else json.dumps(value).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            if sid:
                self.send_header("Mcp-Session-Id", sid)
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            with lock:
                sid = str(len(sessions) + 1)
                state = {"counter": 0, "queue": queue.Queue()}
                sessions[sid] = state
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            self.wfile.write(f"event: endpoint\ndata: /messages?session={sid}\n\n".encode())
            self.wfile.flush()
            try:
                while True:
                    frame = state["queue"].get()
                    self.wfile.write(("event: message\ndata: " + json.dumps(frame) + "\n\n").encode())
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                pass

        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            legacy = self.path.startswith("/messages")
            sid = parse_qs(urlsplit(self.path).query).get("session", [None])[0] if legacy else self.headers.get("Mcp-Session-Id")
            if request["method"] == "initialize" and not legacy:
                with lock:
                    sid = str(len(sessions) + 1)
                    sessions[sid] = {"counter": 0}
            if sid not in sessions:
                self.reply(404)
                return
            if "id" not in request:
                self.reply(202)
                return
            if legacy and request.get("params", {}).get("arguments", {}).get("no_response"):
                self.reply(202)
                return
            frame = response(request, sessions[sid])
            if legacy:
                sessions[sid]["queue"].put(frame)
                self.reply(202)
            else:
                self.reply(200, frame, sid)

        def do_DELETE(self):
            sessions.pop(self.headers.get("Mcp-Session-Id"), None)
            self.reply(200)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(server.server_port, flush=True)
    server.serve_forever()
