#!/usr/bin/env python3
"""Private, bounded slow HTTP and TCP peers for the mixed live soak."""
from __future__ import annotations

import argparse
import http.server
import socketserver
import struct
import threading
import time
import urllib.parse


def checked_size(value: int, maximum: int) -> int:
    if value < 0 or value > maximum:
        raise ValueError(f"size {value} exceeds bounded range 0..{maximum}")
    return value


def body_bytes(size: int) -> bytes:
    alphabet = b"0123456789abcdef"
    return (alphabet * ((size + len(alphabet) - 1) // len(alphabet)))[:size]


def make_http_server(host: str, port: int, *, max_bytes: int, max_delay_ms: int):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.0"

        def log_message(self, fmt: str, *args: object) -> None:
            print("[mixed-slow-peer] http " + (fmt % args), flush=True)

        def do_GET(self) -> None:  # noqa: N802 - stdlib callback name
            parsed = urllib.parse.urlsplit(self.path)
            params = urllib.parse.parse_qs(parsed.query)
            try:
                size = checked_size(int(params.get("bytes", ["4096"])[0]), max_bytes)
                chunks = checked_size(int(params.get("chunks", ["8"])[0]), 64)
                delay_ms = checked_size(int(params.get("delay_ms", ["20"])[0]), max_delay_ms)
                if parsed.path != "/slow" or chunks == 0:
                    raise ValueError("unsupported path or zero chunks")
            except (ValueError, TypeError) as exc:
                self.send_error(400, str(exc))
                return
            body = body_bytes(size)
            self.send_response(200)
            self.send_header("content-type", "application/octet-stream")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            for offset in range(0, len(body), max(1, (len(body) + chunks - 1) // chunks)):
                self.wfile.write(body[offset:offset + max(1, (len(body) + chunks - 1) // chunks)])
                self.wfile.flush()
                time.sleep(delay_ms / 1000)

    class Server(http.server.ThreadingHTTPServer):
        daemon_threads = True
        allow_reuse_address = True

    return Server((host, port), Handler)


def make_tcp_server(host: str, port: int, *, max_bytes: int, read_delay_ms: int):
    class Handler(socketserver.BaseRequestHandler):
        def handle(self) -> None:
            self.request.settimeout(5)
            header = bytearray()
            while len(header) < 4:
                chunk = self.request.recv(4 - len(header))
                if not chunk:
                    return
                header.extend(chunk)
            size = struct.unpack("!I", header)[0]
            try:
                checked_size(size, max_bytes)
            except ValueError:
                self.request.sendall(b"ERR oversized\n")
                return
            received = 0
            while received < size:
                chunk = self.request.recv(min(128, size - received))
                if not chunk:
                    return
                received += len(chunk)
                time.sleep(read_delay_ms / 1000)
            self.request.sendall(f"ACK {received}\n".encode("ascii"))
            print(f"[mixed-slow-peer] tcp bytes={received}", flush=True)

    class Server(socketserver.ThreadingTCPServer):
        daemon_threads = True
        allow_reuse_address = True

    return Server((host, port), Handler)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="0.0.0.0")
    ap.add_argument("--http-port", type=int, default=18080)
    ap.add_argument("--tcp-port", type=int, default=18081)
    ap.add_argument("--max-bytes", type=int, default=65536)
    ap.add_argument("--max-delay-ms", type=int, default=250)
    ap.add_argument("--tcp-read-delay-ms", type=int, default=10)
    args = ap.parse_args()
    if args.max_bytes <= 0 or args.max_delay_ms < 0 or args.tcp_read_delay_ms < 0:
        ap.error("limits must be non-negative and max-bytes must be positive")

    httpd = make_http_server(args.host, args.http_port, max_bytes=args.max_bytes,
                             max_delay_ms=args.max_delay_ms)
    tcpd = make_tcp_server(args.host, args.tcp_port, max_bytes=args.max_bytes,
                           read_delay_ms=args.tcp_read_delay_ms)
    thread = threading.Thread(target=tcpd.serve_forever, daemon=True)
    thread.start()
    print(f"[mixed-slow-peer] ready http={args.http_port} tcp={args.tcp_port} max={args.max_bytes}", flush=True)
    try:
        httpd.serve_forever()
    finally:
        httpd.server_close()
        tcpd.shutdown()
        tcpd.server_close()
        thread.join(2)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
