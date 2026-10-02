#!/usr/bin/env python3
"""Loopback video server with a bounded, private playback telemetry endpoint."""
import argparse
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import re


def byte_range(value, size):
    """One inclusive byte range, or None for an unsatisfiable request."""
    match = re.fullmatch(r"bytes=(\d*)-(\d*)", value.strip())
    if not match or size <= 0 or not any(match.groups()):
        return None
    first, last = match.groups()
    if not first:
        count = int(last)
        return (max(0, size - count), size - 1) if count else None
    start = int(first)
    end = min(int(last), size - 1) if last else size - 1
    return (start, end) if start < size and end >= start else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--events", type=Path, required=True)
    parser.add_argument("--port-file", type=Path, required=True)
    args = parser.parse_args()

    class Handler(SimpleHTTPRequestHandler):
        def __init__(self, *items, **kwargs):
            super().__init__(*items, directory=str(args.directory), **kwargs)

        def send_head(self):
            self.response_range = None
            if self.path.split("?", 1)[0] != "/sample.mp4":
                return super().send_head()
            try:
                source = (args.directory / "sample.mp4").open("rb")
            except OSError:
                self.send_error(404)
                return None
            size = source.seek(0, 2)
            source.seek(0)
            requested = self.headers.get("Range")
            selected = byte_range(requested, size) if requested else None
            if requested and selected is None:
                source.close()
                self.send_response(416)
                self.send_header("Content-Range", f"bytes */{size}")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return None
            self.send_response(206 if selected else 200)
            self.send_header("Content-Type", "video/mp4")
            self.send_header("Accept-Ranges", "bytes")
            if selected:
                start, end = selected
                source.seek(start)
                self.response_range = end - start + 1
                self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
            self.send_header("Content-Length", str(self.response_range if selected else size))
            self.end_headers()
            return source

        def copyfile(self, source, outputfile):
            if self.response_range is None:
                return super().copyfile(source, outputfile)
            remaining = self.response_range
            while remaining:
                block = source.read(min(65536, remaining))
                if not block:
                    break
                outputfile.write(block)
                remaining -= len(block)

        def do_POST(self):
            self.connection.settimeout(5)
            if self.path != "/telemetry":
                self.send_error(404)
                return
            # Only the page served from this listener can post evidence.
            if self.headers.get("Origin") != f"http://127.0.0.1:{self.server.server_port}":
                self.send_error(403)
                return
            try:
                size = int(self.headers.get("Content-Length", "0"))
                if not 0 < size <= 8192 or args.events.stat().st_size > 65536:
                    raise ValueError("telemetry exceeds limit")
                payload = json.loads(self.rfile.read(size))
                if not isinstance(payload, dict):
                    raise ValueError("expected object")
                record = json.dumps(payload, allow_nan=False)
                with args.events.open("a") as events:
                    events.write(record + "\n")
            except (ValueError, OSError, UnicodeDecodeError):
                self.send_error(400)
                return
            self.send_response(204)
            self.end_headers()

    args.events.write_text("")
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.timeout = 1
    args.port_file.write_text(str(server.server_port))
    server.serve_forever()


if __name__ == "__main__":
    main()
