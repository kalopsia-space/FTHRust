#!/usr/bin/env python3
"""A local upload endpoint for the bench: stores each POST body under
DIR and answers {"url": ...} like a custom upload server.

usage: upload_server.py DIR PORT_FILE
"""
import http.server
import json
import pathlib
import sys

DIR = pathlib.Path(sys.argv[1])
DIR.mkdir(parents=True, exist_ok=True)


class Handler(http.server.BaseHTTPRequestHandler):
    count = 0

    def do_HEAD(self):
        self.send_response(200)
        self.end_headers()

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        Handler.count += 1
        (DIR / f"upload-{Handler.count}.bin").write_bytes(body)
        (DIR / f"upload-{Handler.count}.headers").write_text(str(self.headers))
        reply = json.dumps({"url": f"http://127.0.0.1:{self.server.server_port}/f/{Handler.count}.mp4"}).encode()
        self.send_response(201)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(reply)))
        self.end_headers()
        self.wfile.write(reply)

    def log_message(self, *args):
        pass


server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
pathlib.Path(sys.argv[2]).write_text(str(server.server_port))
server.serve_forever()
