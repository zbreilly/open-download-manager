#!/usr/bin/env python3
"""Serve one directory over HTTP with byte-range support, throttled per
connection, so CI can exercise segmented downloads without an outside host.

usage: range_server.py DIR PORT [BYTES_PER_SEC_PER_CONNECTION]
"""
import os
import re
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = os.path.abspath(sys.argv[1])
PORT = int(sys.argv[2])
RATE = int(sys.argv[3]) if len(sys.argv) > 3 else 0
CHUNK = 64 * 1024


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        sys.stderr.write("%s %s\n" % (self.headers.get("Range", "-"), fmt % args))

    def _file(self):
        name = os.path.basename(self.path.split("?", 1)[0])
        p = os.path.join(ROOT, name)
        return p if name and os.path.isfile(p) else None

    def do_HEAD(self):
        self._serve(body=False)

    def do_GET(self):
        self._serve(body=True)

    def _serve(self, body):
        p = self._file()
        if not p:
            self.send_error(404)
            return
        size = os.path.getsize(p)
        start, end, status = 0, size - 1, 200
        m = re.match(r"bytes=(\d*)-(\d*)$", self.headers.get("Range", ""))
        if m and size > 0:
            a, b = m.groups()
            if a == "":
                start = max(0, size - int(b))
            else:
                start = int(a)
                end = min(int(b), size - 1) if b else size - 1
            if start > end:
                self.send_response(416)
                self.send_header("Content-Range", "bytes */%d" % size)
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            status = 206
        self.send_response(status)
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("ETag", '"%x-%x"' % (size, int(os.path.getmtime(p))))
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(end - start + 1))
        if status == 206:
            self.send_header("Content-Range", "bytes %d-%d/%d" % (start, end, size))
        self.end_headers()
        if not body:
            return
        with open(p, "rb") as f:
            f.seek(start)
            left = end - start + 1
            t0 = time.monotonic()
            sent = 0
            while left > 0:
                data = f.read(min(CHUNK, left))
                if not data:
                    break
                try:
                    self.wfile.write(data)
                except (BrokenPipeError, ConnectionResetError):
                    return
                left -= len(data)
                sent += len(data)
                if RATE:
                    ahead = sent / RATE - (time.monotonic() - t0)
                    if ahead > 0:
                        time.sleep(ahead)


ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
