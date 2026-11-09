"""Compare UDP receive windows with paced TCP and simulated credit return delay."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import math
from pathlib import Path
import queue
import socket
import struct
import subprocess
import threading
import time

from test_echo import exact


BASE_BYTES = 64 * 1024
BASE_SLOTS = 64
TCP_BYTES = 4 * 1024 * 1024
TCP_CHUNK = bytes(16384)
PROFILES = {
    "baseline": (BASE_BYTES, BASE_SLOTS),
    "bytes": (2 * BASE_BYTES, BASE_SLOTS),
    "slots": (BASE_BYTES, 2 * BASE_SLOTS),
    "both": (2 * BASE_BYTES, 2 * BASE_SLOTS),
}


def until(deadline):
    time.sleep(max(0, deadline - time.monotonic()))


def target(sock):
    return b"\0\0\0\1\x7f\0\0\1" + struct.pack("!H", sock.getsockname()[1])


class Agent:
    def __init__(self, binary, udp_window, credit_delay):
        self.process = subprocess.Popen(
            [binary, "--stdio-v3-flow-control"], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        )
        self.udp_window = udp_window
        self.credit_delay = credit_delay
        self.write_lock = threading.Lock()
        self.credit = queue.Queue()
        self.replies = queue.Queue()
        self.stats = queue.Queue()
        self.udp = {}
        self.tcp = {}
        self.workers = [
            threading.Thread(target=self.read, daemon=True),
            threading.Thread(target=self.return_credit, daemon=True),
        ]

    def __enter__(self):
        self.workers[0].start()
        try:
            kind, _, payload = self.replies.get(timeout=5)
            if kind != "hello":
                raise RuntimeError(f"Agent handshake failed: {payload}")
            self.process.stdin.write(
                b"STMUX003" + struct.pack("!IIIII", 32, BASE_BYTES, BASE_SLOTS, *self.udp_window)
            )
            self.process.stdin.flush()
            self.workers[1].start()
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def __exit__(self, *_):
        self.process.kill()
        self.process.wait(timeout=5)
        self.credit.put(None)
        for worker in self.workers:
            if worker.ident is not None:
                worker.join(timeout=2)
        self.process.stdin.close()
        self.process.stdout.close()

    def send(self, kind, flow, data=b""):
        with self.write_lock:
            self.process.stdin.write(struct.pack("!HBI", len(data) + 5, kind, flow) + data)
            self.process.stdin.flush()

    def read(self):
        try:
            if exact(self.process.stdout, 28)[:8] != b"STMUX003":
                raise ValueError("Unexpected protocol")
            self.replies.put(("hello", 0, None))
            while True:
                length, = struct.unpack("!H", exact(self.process.stdout, 2))
                frame = exact(self.process.stdout, length)
                kind, flow = struct.unpack("!BI", frame[:5])
                data = frame[5:]
                now = time.monotonic()
                if kind == 5:
                    if flow in self.udp:
                        seq, = struct.unpack("!I", data[10:14])
                        self.udp[flow][seq] = now
                    else:
                        previous, _ = self.tcp[flow]
                        self.tcp[flow] = (previous + len(data), now)
                    self.credit.put((now + self.credit_delay, flow, len(data)))
                elif kind in (3, 4):
                    self.replies.put((kind, flow, data))
                elif kind == 10 and data[96:101] == b"\xffSTD1":
                    self.stats.put(json.loads(data[101:])["metrics"])
        except (EOFError, ValueError, KeyError, OSError) as error:
            self.replies.put(("error", 0, str(error)))

    def return_credit(self):
        while item := self.credit.get():
            deadline, flow, size = item
            until(deadline)
            try:
                self.send(8, flow, struct.pack("!II", size, 1))
            except (BrokenPipeError, OSError):
                break

    def open(self, kind, flow, destination=b"", timeout=12):
        self.send(kind, flow, destination)
        reply, reply_flow, reason = self.replies.get(timeout=timeout)
        if reply_flow != flow or reply not in (3, 4):
            raise RuntimeError(f"Unexpected response for flow {flow}: {reply, reply_flow, reason}")
        return reply, reason

    def report_after(self, attempts):
        end = time.monotonic() + 8
        while True:
            stats = self.stats.get(timeout=max(0, end - time.monotonic()))
            if stats["udp_tx_attempts"] >= attempts:
                return stats


def paced_tcp(peer, start):
    with peer:
        for index in range(TCP_BYTES // len(TCP_CHUNK)):
            until(start + index * .020)
            peer.sendall(TCP_CHUNK)
        peer.shutdown(socket.SHUT_WR)


def paced_udp(sock, peer, start, bursts, count, size, gap, interval):
    sent = {}
    for burst in range(bursts):
        until(start + burst * interval)
        for index in range(count):
            seq = burst * count + index
            sent[seq] = time.monotonic()
            sock.sendto(struct.pack("!I", seq) + bytes(size - 4), peer)
            if gap:
                time.sleep(gap)
    return sent


def udp_summary(sent, received):
    latencies = sorted(1000 * (received[seq] - ts) for seq, ts in sent.items() if seq in received)
    return {
        "sent": len(sent), "received": len(latencies),
        "loss_pct": round(100 * (1 - len(latencies) / len(sent)), 3),
        **{
            f"p{percentile}_ms": round(latencies[math.ceil(len(latencies) * percentile / 100) - 1], 3)
            if latencies else None
            for percentile in (50, 95, 99)
        },
    }


def benchmark(binary, window, delay):
    udp_flows = {"voice": 1, "burst_1200": 3, "burst_160": 5}
    with Agent(binary, window, delay) as agent, socket.socket() as tcp, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
        udp.bind(("127.0.0.1", 0))
        udp.settimeout(4)
        tcp.bind(("127.0.0.1", 0))
        tcp.listen()
        tcp.settimeout(4)
        sources = {}
        for name, flow in udp_flows.items():
            agent.udp[flow] = {}
            assert agent.open(2, flow)[0] == 3
            agent.send(5, flow, target(udp) + b"ready")
            payload, sources[name] = udp.recvfrom(32)
            assert payload == b"ready"
        backends = []
        for flow in (7, 9, 11, 13):
            agent.tcp[flow] = (0, 0)
            assert agent.open(1, flow, target(tcp))[0] == 3
            peer, _ = tcp.accept()
            peer.settimeout(10)
            backends.append(peer)
            agent.send(6, flow)
        with ThreadPoolExecutor(max_workers=7) as pool:
            start = time.monotonic() + .12
            tcp_tasks = [pool.submit(paced_tcp, peer, start) for peer in backends]
            udp_tasks = {
                "voice": pool.submit(paced_udp, udp, sources["voice"], start, 250, 1, 160, 0, .020),
                "burst_1200": pool.submit(paced_udp, udp, sources["burst_1200"], start + .05, 7, 88, 1200, .00030, .68),
                "burst_160": pool.submit(paced_udp, udp, sources["burst_160"], start + .28, 7, 88, 160, .00030, .68),
            }
            sent = {name: task.result(timeout=10) for name, task in udp_tasks.items()}
            for task in tcp_tasks:
                task.result(timeout=10)
        deadline = time.monotonic() + 6
        while sum(x[0] for x in agent.tcp.values()) < TCP_BYTES * 4:
            if time.monotonic() >= deadline:
                raise RuntimeError("Incomplete TCP transfer")
            time.sleep(.010)
        tcp_seconds = max(x[1] for x in agent.tcp.values()) - start
        stats = agent.report_after(sum(map(len, sent.values())))
        process_status = Path(f"/proc/{agent.process.pid}/status")
        peak_rss = None
        if process_status.is_file():
            peak_rss = next(
                (int(line.split()[1]) for line in process_status.read_text().splitlines()
                 if line.startswith("VmHWM:")), None,
            )
        return {
            "udp": {name: udp_summary(sent[name], agent.udp[flow]) for name, flow in udp_flows.items()},
            "tcp_mbps": round(4 * TCP_BYTES * 8 / tcp_seconds / 1e6, 3),
            "udp_tx_attempts": stats["udp_tx_attempts"],
            "credit_bytes": stats["udp_credit_bytes"],
            "credit_slots": stats["udp_credit_slots"],
            "credit_both": stats["udp_credit_both"],
            "credit_return": stats["udp_credit_return"],
            "agent_peak_rss_kib": peak_rss,
        }


def check_timeout(binary):
    """Hold a Linux loopback listener's accept backlog full."""
    with socket.socket() as listener, Agent(binary, PROFILES["baseline"], 0) as agent:
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        pending = []
        try:
            for _ in range(8):
                try:
                    pending.append(socket.create_connection(listener.getsockname(), timeout=.2))
                except TimeoutError:
                    break
            else:
                raise RuntimeError("Could not fill the loopback accept backlog")
            start = time.monotonic()
            kind, reason = agent.open(1, 1, target(listener), timeout=13)
            elapsed = time.monotonic() - start
            assert kind == 4 and reason == b"TCP connect timeout", (kind, reason)
            assert 9.8 <= elapsed < 12, elapsed
            print(json.dumps({"tcp_timeout": reason.decode(), "elapsed_seconds": round(elapsed, 3)}), flush=True)
        finally:
            for peer in pending:
                peer.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("agent")
    parser.add_argument("--profile", choices=PROFILES, action="append")
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--credit-delay-ms", type=float, default=53)
    parser.add_argument("--check-timeout", action="store_true")
    args = parser.parse_args()
    if args.repetitions < 1 or not 0 <= args.credit_delay_ms <= 1000:
        parser.error("Use at least one repetition and a credit delay between 0 and 1000 ms")
    for repetition in range(args.repetitions):
        for profile in args.profile or PROFILES:
            result = benchmark(args.agent, PROFILES[profile], args.credit_delay_ms / 1000)
            print(json.dumps({
                "repetition": repetition + 1, "profile": profile,
                "credit_delay_ms": args.credit_delay_ms, **result,
            }), flush=True)
    if args.check_timeout:
        check_timeout(args.agent)


if __name__ == "__main__":
    main()
