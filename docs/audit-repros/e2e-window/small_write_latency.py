"""Latency of two back-to-back small server writes, direct vs through SOCKS.

A loopback server sends message A, waits 2 ms, sends message B, then idles.
The receiver never writes, so a sender with Nagle enabled holds B until A is
acknowledged (delayed ACK). Compares a direct connection with the tunnel's
local SOCKS listener: python3 small_write_latency.py SOCKS_PORT
"""
import socket
import statistics
import struct
import sys
import threading
import time

SIZE, ROUNDS = 108, 60


def server(listener):
    client, _ = listener.accept()
    with client:
        client.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        time.sleep(0.5)
        for _ in range(ROUNDS):
            for _ in range(2):
                client.sendall(struct.pack("!d", time.monotonic()) + bytes(SIZE - 8))
                time.sleep(0.002)
            time.sleep(0.15)


def run(socks_port):
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    port = listener.getsockname()[1]
    threading.Thread(target=server, args=(listener,), daemon=True).start()
    if socks_port:
        client = socket.create_connection(("127.0.0.1", socks_port))
        client.sendall(b"\x05\x01\x00")
        assert client.recv(2) == b"\x05\x00"
        client.sendall(b"\x05\x01\x00\x01" + socket.inet_aton("127.0.0.1") + port.to_bytes(2, "big"))
        reply = b""
        while len(reply) < 10:
            reply += client.recv(10 - len(reply))
        assert reply[1] == 0
    else:
        client = socket.create_connection(("127.0.0.1", port))
    latencies = ([], [])
    buffer = b""
    index = 0
    while index < ROUNDS * 2:
        data = client.recv(65536)
        if not data:
            break
        now = time.monotonic()
        buffer += data
        while len(buffer) >= SIZE:
            sent = struct.unpack("!d", buffer[:8])[0]
            latencies[index % 2].append((now - sent) * 1000)
            buffer = buffer[SIZE:]
            index += 1
    client.close()
    return latencies


def describe(values):
    values = sorted(values)
    return f"median {statistics.median(values):.2f} ms, p95 {values[int(len(values) * 0.95) - 1]:.2f} ms, max {values[-1]:.2f} ms"


if __name__ == "__main__":
    for name, port in (("direct", 0), ("through SOCKS", int(sys.argv[1]))):
        first, second = run(port)
        print(f"{name}: first write {describe(first)}; second write {describe(second)}")
