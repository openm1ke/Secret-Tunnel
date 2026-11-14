"""Ephemeral loopback-only benchmark endpoints. Run under the SSH user's UID.

Prints JSON ports, exits after --seconds. Creates no files or services.
"""
import argparse
import hashlib
import json
import os
import socket
import threading
import time

BLOCK = bytes(range(256)) * 64
TCP_BYTES = 2 * 1024 * 1024


def transfer(client):
    with client:
        client.settimeout(45)
        try:
            request = b""
            while not request.endswith(b"\n") and len(request) < 64:
                part = client.recv(1)
                if not part:
                    return
                request += part
            if request.startswith(b"SINK ") and request[5:-1].isdigit():
                remaining = int(request[5:-1])
                if not 65536 <= remaining <= 8 * 1024 * 1024:
                    return
                digest = hashlib.sha256()
                while remaining:
                    block = client.recv(min(65536, remaining))
                    if not block:
                        return
                    digest.update(block)
                    remaining -= len(block)
                client.sendall(digest.digest())
                return
            if request == b"PAYLOAD\n":
                size = TCP_BYTES
            elif request.startswith(b"PAYLOAD ") and request[8:-1].isdigit():
                size = int(request[8:-1])
            else:
                return
            if size % len(BLOCK) or not 65536 <= size <= 8 * 1024 * 1024:
                return
            for _ in range(size // len(BLOCK)):
                client.sendall(BLOCK)
                time.sleep(.005)
        except OSError:
            pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--seconds", type=int, default=1800)
    args = parser.parse_args()
    assert 1 <= args.seconds <= 7200
    with socket.socket() as tcp, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
        tcp.bind(("127.0.0.1", 0))
        tcp.listen(16)
        udp.bind(("127.0.0.1", 0))
        tcp.settimeout(.5)
        udp.settimeout(.5)
        deadline = time.monotonic() + args.seconds

        def accept():
            while time.monotonic() < deadline:
                try:
                    client, _ = tcp.accept()
                except socket.timeout:
                    continue
                threading.Thread(target=transfer, args=(client,), daemon=True).start()

        threading.Thread(target=accept, daemon=True).start()
        print(json.dumps({"pid": os.getpid(), "tcp_port": tcp.getsockname()[1], "udp_port": udp.getsockname()[1]}), flush=True)
        while time.monotonic() < deadline:
            try:
                data, peer = udp.recvfrom(65507)
                if len(data) in (160, 1200):
                    udp.sendto(data, peer)
            except socket.timeout:
                pass


if __name__ == "__main__":
    main()
