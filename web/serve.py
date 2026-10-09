#!/usr/bin/env python3
"""Serves web/dist on http://localhost:8080 (`python3 serve.py [PORT]`).

Sends the precompressed `.br` / `.gz` copy of a file when the browser accepts it, and caches
the game packs for good: the page asks for them as `?v=CRC`, so a changed pack has a new URL.
localhost counts as a secure context, which WebGPU needs; another host needs HTTPS.
"""
import http.server, os, sys

TYPES = {".wasm": "application/wasm", ".js": "text/javascript", ".html": "text/html",
         ".json": "application/json", ".gz": "application/octet-stream"}


class Handler(http.server.SimpleHTTPRequestHandler):
    def send_head(self):
        path = self.translate_path(self.path.split("?")[0])
        if os.path.isdir(path):
            path = os.path.join(path, "index.html")
        accept = self.headers.get("Accept-Encoding", "")
        sent, enc = path, None
        for ext, name in ((".br", "br"), (".gz", "gzip")):
            # a pack is already gzip: the page inflates it itself
            if name in accept and not path.endswith(".gz") and os.path.isfile(path + ext):
                sent, enc = path + ext, name
                break
        if not os.path.isfile(sent):
            self.send_error(404)
            return None
        f = open(sent, "rb")
        self.send_response(200)
        self.send_header("Content-Type", TYPES.get(os.path.splitext(path)[1], "application/octet-stream"))
        self.send_header("Content-Length", str(os.fstat(f.fileno()).st_size))
        if enc:
            self.send_header("Content-Encoding", enc)
        self.send_header("Vary", "Accept-Encoding")
        immutable = "?v=" in self.path
        self.send_header("Cache-Control", "public, max-age=31536000, immutable" if immutable else "no-cache")
        self.end_headers()
        return f


os.chdir(os.path.dirname(os.path.abspath(__file__)))
port = int(sys.argv[1]) if len(sys.argv) > 1 else 8080
print(f"http://localhost:{port}")
http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
