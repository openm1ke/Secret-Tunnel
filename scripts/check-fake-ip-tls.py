#!/usr/bin/env python3
"""Regress fake-IP CONNECT with real TLS verification and Discord update URLs.

The SOCKS request deliberately contains a synthetic IP, while the unchanged TLS
ClientHello carries the actual destination. Run against an embedded listener.
"""
import socket
import ssl
import struct
import sys


def exact(stream, size):
    result = b""
    while len(result) < size:
        chunk = stream.recv(size - len(result))
        if not chunk:
            raise RuntimeError("SOCKS connection closed")
        result += chunk
    return result


def check(port, host, path):
    raw = socket.create_connection(("127.0.0.1", port), timeout=12)
    try:
        raw.sendall(b"\x05\x01\x00")
        assert exact(raw, 2) == b"\x05\x00"
        raw.sendall(b"\x05\x01\x00\x01" + socket.inet_aton("198.18.0.254") + struct.pack("!H", 443))
        header = exact(raw, 4)
        assert header[:3] == b"\x05\x00\x00", header
        if header[3] == 1:
            exact(raw, 6)
        elif header[3] == 4:
            exact(raw, 18)
        else:
            exact(raw, exact(raw, 1)[0] + 2)
        with ssl.create_default_context().wrap_socket(raw, server_hostname=host) as stream:
            stream.sendall(f"GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n".encode())
            headers = b""
            while b"\r\n\r\n" not in headers:
                part = stream.recv(4096)
                if not part or len(headers) > 32768:
                    raise RuntimeError("Invalid HTTP response")
                headers += part
            status = headers.split(b"\r\n", 1)[0].decode()
            assert status.split()[1] == "200", status
            print(f"PASS: fake-IP -> TLS SNI -> {host}, certificate verified, {status}", flush=True)
    finally:
        raw.close()


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 1082
    check(port, "discord.com", "/api/updates/stable?platform=osx")
    check(port, "updates.discord.com", "/distributions/app/manifests/latest?install_id=0&channel=stable&platform=osx&arch=arm64")
