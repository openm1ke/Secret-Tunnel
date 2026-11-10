#!/usr/bin/env python3
"""Measure UDP delay caused by a slow lookup within one mux association.

Creates a private Linux network and mount namespace for the loopback DNS and
UDP echo fixtures. The production agent runs over stdio (see README.md).
"""

import argparse
import json
import os
from pathlib import Path
import queue
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time


DELAY_SECONDS = 1.2
UDP_WINDOW = (128 * 1024, 128)


def exact(stream, size):
    result = bytearray()
    while len(result) < size:
        chunk = stream.read(size - len(result))
        if not chunk:
            raise EOFError("Agent closed its output")
        result.extend(chunk)
    return bytes(result)


def frame(kind, flow, data=b""):
    return struct.pack("!HBI", len(data) + 5, kind, flow) + data


def experiment(binary, repeats, echo, dns_events, cancelled_packet):
    agent = subprocess.Popen(
        [binary, "--stdio-v3-qos"], stdin=subprocess.PIPE, stdout=subprocess.PIPE
    )
    frames = queue.Queue()

    def read_frames():
        try:
            hello = exact(agent.stdout, 28)
            if hello[:8] != b"STMUX003":
                raise ValueError("Unexpected agent handshake")
            while True:
                size, kind, flow = struct.unpack("!HBI", exact(agent.stdout, 7))
                frames.put((kind, flow, exact(agent.stdout, size - 5), time.monotonic()))
        except (EOFError, ValueError) as error:
            frames.put(error)

    def send(data):
        agent.stdin.write(data)
        agent.stdin.flush()

    def receive(deadline):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("Timed out waiting for an agent frame")
        item = frames.get(timeout=remaining)
        if isinstance(item, Exception):
            raise item
        return item

    reader = threading.Thread(target=read_frames, daemon=True)
    reader.start()
    target = b"\x00\x00\x00\x01\x7f\x00\x00\x01" + struct.pack("!H", echo.getsockname()[1])

    def ping_pair(sequence):
        stamp = time.monotonic()
        marker = struct.pack("!I", sequence)
        send(frame(5, 1, target + marker) + frame(5, 3, target + marker))
        replies = {}
        while len(replies) < 2:
            kind, flow, payload, when = receive(stamp + 5)
            if kind != 5:
                continue
            assert flow in (1, 3) and payload == target + marker
            replies[flow] = round((when - stamp) * 1000, 2)
            send(frame(8, flow, struct.pack("!II", len(payload), 1)))
        return replies

    try:
        send(b"STQOS001" + struct.pack("!IHH", *UDP_WINDOW, 96))
        send(b"STMUX003" + struct.pack("!IIIII", 64, 65536, 64, *UDP_WINDOW))
        send(frame(2, 1) + frame(2, 3))
        ready = set()
        deadline = time.monotonic() + 5
        while ready != {1, 3}:
            kind, flow, _, _ = receive(deadline)
            if kind == 3:
                ready.add(flow)
        control = ping_pair(0)
        samples = []
        for sequence in range(1, repeats + 1):
            name = f"slow-{sequence}.test".encode("ascii")
            domain_target = b"\x00\x00\x00\x03" + bytes([len(name)]) + name + target[-2:]
            send(frame(5, 1, domain_target + b"dns-fixture"))
            assert dns_events.get(timeout=4) == name
            replies = ping_pair(sequence)
            samples.append({"same_association_ms": replies[1], "other_association_ms": replies[3]})

        name = b"cancel.test"
        domain_target = b"\x00\x00\x00\x03" + bytes([len(name)]) + name + target[-2:]
        send(frame(5, 1, domain_target + b"cancel-fixture"))
        assert dns_events.get(timeout=4) == name
        # Acceptance on flow 5 proves the reader has already processed Abort.
        send(frame(9, 1) + frame(2, 5))
        deadline = time.monotonic() + 5
        while receive(deadline)[:2] != (3, 5):
            pass
        sent_after_abort = cancelled_packet.wait(DELAY_SECONDS + 1)
        agent.stdin.close()
        assert agent.wait(timeout=5) == 0
        return {
            "dns_delay_ms": round(DELAY_SECONDS * 1000),
            "control_ms": control,
            "samples": samples,
            "udp_sent_after_abort": sent_after_abort,
        }
    finally:
        if agent.poll() is None:
            agent.kill()
            agent.wait(timeout=5)
        reader.join(timeout=1)
        agent.stdin.close()
        agent.stdout.close()


def serve_dns(dns, events, stopped):
    first = {}
    pending = []
    while not stopped.is_set():
        now = time.monotonic()
        for due, reply, source in pending[:]:
            if due <= now:
                dns.sendto(reply, source)
                pending.remove((due, reply, source))
        try:
            query, source = dns.recvfrom(512)
        except socket.timeout:
            continue
        offset, labels = 12, []
        while query[offset]:
            size = query[offset]
            labels.append(query[offset + 1 : offset + 1 + size])
            offset += 1 + size
        name = b".".join(labels)
        if name not in first:
            first[name] = time.monotonic()
            events.put(name)
        question = query[12 : offset + 5]
        if name == b"cancel.test":
            # Return only an A record so the cancelled packet has an egress.
            answer = b""
            if question[-4:-2] == b"\x00\x01":
                answer = b"\xc0\x0c" + struct.pack("!HHIH", 1, 1, 1, 4) + socket.inet_aton("127.0.0.1")
            reply = query[:2] + struct.pack("!5H", 0x8180, 1, bool(answer), 0, 0) + question + answer
        else:
            reply = query[:2] + struct.pack("!5H", 0x8183, 1, 0, 0, 0) + question
        pending.append((first[name] + DELAY_SECONDS, reply, source))


def serve_echo(echo, stopped, cancelled_packet):
    while not stopped.is_set():
        try:
            packet, source = echo.recvfrom(65535)
        except socket.timeout:
            continue
        if packet == b"cancel-fixture":
            cancelled_packet.set()
        echo.sendto(packet, source)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("agent", type=Path)
    parser.add_argument("--repeats", type=int, choices=range(1, 11), default=5)
    parser.add_argument("--parent-namespaces", nargs=2, help=argparse.SUPPRESS)
    args = parser.parse_args()
    namespaces = [os.readlink(f"/proc/self/ns/{name}") for name in ("net", "mnt")]
    if args.parent_namespaces is None:
        subprocess.run(
            ["unshare", "--user", "--map-root-user", "--mount", "--net", "--propagation", "private",
             sys.executable, str(Path(__file__).resolve()), str(args.agent.resolve()),
             "--repeats", str(args.repeats), "--parent-namespaces", *namespaces],
            check=True,
        )
        return
    if any(current == original for current, original in zip(namespaces, args.parent_namespaces)):
        parser.error("Both network and mount namespaces must be isolated")
    subprocess.run(["ip", "link", "set", "lo", "up"], check=True)
    with tempfile.TemporaryDirectory(prefix="st-dns-fixture-") as temp:
        resolv = Path(temp) / "resolv.conf"
        resolv.write_text("nameserver 127.0.0.1\noptions timeout:2 attempts:1\n", encoding="ascii")
        subprocess.run(["mount", "--bind", str(resolv), "/etc/resolv.conf"], check=True)
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as dns, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as echo:
                dns.bind(("127.0.0.1", 53))
                echo.bind(("127.0.0.1", 0))
                dns.settimeout(0.01)
                echo.settimeout(0.05)
                events, stopped, cancelled_packet = queue.Queue(), threading.Event(), threading.Event()
                fixtures = [
                    threading.Thread(target=serve_dns, args=(dns, events, stopped)),
                    threading.Thread(target=serve_echo, args=(echo, stopped, cancelled_packet)),
                ]
                for fixture in fixtures:
                    fixture.start()
                try:
                    result = experiment(str(args.agent.resolve()), args.repeats, echo, events, cancelled_packet)
                finally:
                    stopped.set()
                    for fixture in fixtures:
                        fixture.join()
            print(json.dumps(result, indent=2))
        finally:
            subprocess.run(["umount", "/etc/resolv.conf"], check=True)


if __name__ == "__main__":
    main()
