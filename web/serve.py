#!/usr/bin/env python3
"""Serves web/dist on http://127.0.0.1:8080 (`python3 serve.py [PORT]`).

Sends the precompressed `.br` / `.gz` copy of a file when the browser accepts it, with the
unpacked size in `X-Raw-Length` (the page's progress bars count unpacked bytes), and caches
the game packs for good: the page asks for them as `?v=CRC`, so a changed pack has a new URL.
`data/files/` holds only `.gz` copies (the clothes the game downloads one by one); they are
sent as gzip and cached for a day.
localhost counts as a secure context, which WebGPU needs; another host needs HTTPS, e.g.
`tailscale serve --bg --https=10000 http://127.0.0.1:8080` in front of this.
`POST /log` prints the page's reports (errors, warnings and GPU details of the device it runs
on, one line per request) to stderr: the way to see what went wrong on a phone or tablet.
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
        # a file kept only gzipped goes out as gzip to every browser
        if not os.path.isfile(sent) and os.path.isfile(path + ".gz"):
            sent, enc = path + ".gz", "gzip"
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
        if os.path.isfile(path):
            self.send_header("X-Raw-Length", str(os.path.getsize(path)))
        if "?v=" in self.path:
            cache = "public, max-age=31536000, immutable"
        elif "/data/files/" in self.path:
            cache = "public, max-age=86400"
        else:
            cache = "no-cache"
        self.send_header("Cache-Control", cache)
        self.end_headers()
        return f

    def do_POST(self):
        if self.path != "/log":
            self.send_error(404)
            return
        n = min(int(self.headers.get("Content-Length") or 0), 8192)
        line = self.rfile.read(n).decode("utf-8", "replace").replace("\n", " | ")
        sys.stderr.write(f"page: {line}\n")
        self.send_response(204)
        self.end_headers()

    def log_request(self, code="-", size="-"):
        if self.path != "/log":
            super().log_request(code, size)


os.chdir(os.path.dirname(os.path.abspath(__file__)))
port = int(sys.argv[1]) if len(sys.argv) > 1 else 8080
print(f"http://localhost:{port}")
http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
