"""End-to-end check of the per-flow window / RTT limit through the real stack.

  Python client -> SOCKS5 (check_route: shared Rust SOCKS + stream-mux + russh)
    -> [delay proxy] -> sshd -> secret-tunnel-agent -> loopback TCP source/sink

Only loopback, a throwaway sshd instance and throwaway keys are used. The delay
proxy adds a fixed one-way delay to the encrypted SSH byte stream; it has no
rate limit and never drops bytes, so it is not a packet-loss emulator.

See README.md in this directory for the fixture and the exact command.
"""
import argparse
import asyncio
import json
import os
import socket
import subprocess
import tempfile
import threading
import time

ZEROS = bytes(1 << 18)


class DelayProxy:
    """TCP relay with an adjustable one-way delay, an optional rate limit per
    direction and an unbounded FIFO, like a deep-buffered bottleneck link."""

    def __init__(self, host, port):
        self.target = (host, port)
        self.delay = 0.0
        self.rate = 0.0  # bytes per second in each direction; 0 is unlimited
        self.max_wait = 0.0  # longest time a chunk queued behind the rate limit
        self.loop = asyncio.new_event_loop()
        threading.Thread(target=self.loop.run_forever, daemon=True).start()
        server = asyncio.run_coroutine_threadsafe(self._start(), self.loop).result(5)
        self.port = server.sockets[0].getsockname()[1]

    async def _start(self):
        return await asyncio.start_server(self._accept, "127.0.0.1", 0)

    async def _pipe(self, reader, writer):
        queue = asyncio.Queue()

        async def read():
            while True:
                data = await reader.read(4096 if self.rate else 65536)
                await queue.put((time.monotonic() + self.delay, data))
                if not data:
                    return

        task = asyncio.create_task(read())
        available = 0.0
        try:
            while True:
                due, data = await queue.get()
                wait = max(due, available) - time.monotonic()
                if wait > 0:
                    await asyncio.sleep(wait)
                if not data:
                    break
                now = time.monotonic()
                self.max_wait = max(self.max_wait, now - due)
                if self.rate:
                    available = max(available, now) + len(data) / self.rate
                writer.write(data)
                await writer.drain()
        except Exception:
            pass
        finally:
            task.cancel()
            writer.close()

    async def _accept(self, reader, writer):
        try:
            upstream_reader, upstream_writer = await asyncio.open_connection(*self.target)
        except OSError:
            writer.close()
            return
        for stream in (writer, upstream_writer):
            stream.get_extra_info("socket").setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        await asyncio.gather(
            self._pipe(reader, upstream_writer), self._pipe(upstream_reader, writer)
        )


class Endpoints:
    """Loopback TCP source (sends zeros) and sink (counts received bytes)."""

    def __init__(self):
        self.received = 0
        self.lock = threading.Lock()
        self.source_port = self._listen(self._source)
        self.sink_port = self._listen(self._sink)

    def _listen(self, handler):
        server = socket.socket()
        server.bind(("127.0.0.1", 0))
        server.listen(64)

        def accept():
            while True:
                client, _ = server.accept()
                threading.Thread(target=handler, args=(client,), daemon=True).start()

        threading.Thread(target=accept, daemon=True).start()
        return server.getsockname()[1]

    def _source(self, client):
        with client:
            try:
                while True:
                    client.sendall(ZEROS)
            except OSError:
                pass

    def _sink(self, client):
        with client:
            try:
                while True:
                    data = client.recv(1 << 18)
                    if not data:
                        return
                    with self.lock:
                        self.received += len(data)
            except OSError:
                pass


def socks_connect(socks_port, port):
    client = socket.create_connection(("127.0.0.1", socks_port), timeout=20)
    client.sendall(b"\x05\x01\x00")
    assert client.recv(2) == b"\x05\x00"
    client.sendall(b"\x05\x01\x00\x01" + socket.inet_aton("127.0.0.1") + port.to_bytes(2, "big"))
    reply = b""
    while len(reply) < 10:
        reply += client.recv(10 - len(reply))
    assert reply[1] == 0, f"SOCKS CONNECT failed: {reply[1]}"
    client.settimeout(None)
    return client


DNS_QUERY = (b"\x12\x34\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00"
             b"\x07example\x03com\x00\x00\x01\x00\x01")


def dns_lookup(socks_port):
    """One lookup the way a TUN adapter sends it: its own UDP association."""
    control = socket.create_connection(("127.0.0.1", socks_port), timeout=5)
    try:
        control.sendall(b"\x05\x01\x00")
        control.recv(2)
        control.sendall(b"\x05\x03\x00\x01\x00\x00\x00\x00\x00\x00")
        reply = b""
        while len(reply) < 10:
            reply += control.recv(10 - len(reply))
        if reply[1] != 0:
            return False
        relay = (socket.inet_ntoa(reply[4:8]), int.from_bytes(reply[8:10], "big"))
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
            udp.settimeout(2)
            udp.sendto(b"\x00\x00\x00\x01" + socket.inet_aton("1.1.1.1") + (53).to_bytes(2, "big") + DNS_QUERY, relay)
            udp.recvfrom(2048)
        return True
    except OSError:
        return False
    finally:
        control.close()


def dns_bursts(socks_port, lookups, stop, answered):
    """Every two seconds resolve `lookups` names at once, as a page load does."""
    while not stop.is_set():
        threads = [threading.Thread(target=lambda: answered.append(dns_lookup(socks_port)), daemon=True)
                   for _ in range(lookups)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(3)
        stop.wait(2)


class Voice:
    """160-byte UDP datagrams at 50/s through a SOCKS UDP association to a
    loopback echo, like the team's voice stand; records round trips in ms."""

    def __init__(self, socks_port):
        self.echo = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.echo.bind(("127.0.0.1", 0))
        threading.Thread(target=self._echo, daemon=True).start()
        self.control = socket.create_connection(("127.0.0.1", socks_port), timeout=10)
        self.control.sendall(b"\x05\x01\x00")
        assert self.control.recv(2) == b"\x05\x00"
        self.control.sendall(b"\x05\x03\x00\x01\x00\x00\x00\x00\x00\x00")
        reply = b""
        while len(reply) < 10:
            reply += self.control.recv(10 - len(reply))
        assert reply[1] == 0, "UDP ASSOCIATE refused"
        self.relay = (socket.inet_ntoa(reply[4:8]), int.from_bytes(reply[8:10], "big"))
        self.udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.udp.settimeout(0.2)
        self.header = b"\x00\x00\x00\x01" + socket.inet_aton("127.0.0.1") + self.echo.getsockname()[1].to_bytes(2, "big")
        self.sent, self.rtts, self.recording = {}, [], False
        self.stop = threading.Event()
        threading.Thread(target=self._send, daemon=True).start()
        threading.Thread(target=self._receive, daemon=True).start()

    def _echo(self):
        while True:
            data, peer = self.echo.recvfrom(2048)
            self.echo.sendto(data, peer)

    def _send(self):
        sequence = 0
        while not self.stop.is_set():
            sequence += 1
            self.sent[sequence] = (time.monotonic(), self.recording)
            payload = sequence.to_bytes(4, "big") + bytes(156)
            self.udp.sendto(self.header + payload, self.relay)
            time.sleep(0.02)

    def _receive(self):
        while not self.stop.is_set():
            try:
                data, _ = self.udp.recvfrom(2048)
            except OSError:
                continue
            sequence = int.from_bytes(data[10:14], "big")
            sent = self.sent.pop(sequence, None)
            if sent and sent[1]:
                self.rtts.append((time.monotonic() - sent[0]) * 1000)

    def window(self):
        self.rtts, self.recording = [], True
        sent_before = len([1 for _, recording in self.sent.values() if recording])
        return sent_before

    def result(self):
        self.recording = False
        values = sorted(self.rtts)
        late = len([1 for _, recording in self.sent.values() if recording])
        if not values:
            return {"received": 0, "missing": late}
        return {"received": len(values), "missing": late,
                "p50_ms": round(values[len(values) // 2], 1),
                "p95_ms": round(values[max(0, int(len(values) * 0.95) - 1)], 1),
                "within_150ms": len([v for v in values if v <= 150])}

    def close(self):
        self.stop.set()
        self.control.close()


def cpu_seconds(pids):
    total = 0.0
    for pid in pids:
        out = subprocess.run(["ps", "-o", "time=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
        if out:
            parts = out.split(":")
            total += sum(float(part) * 60 ** index for index, part in enumerate(reversed(parts)))
    return total


def measure(socks_port, endpoints, flows, direction, warmup, seconds, voice=None):
    counted = [0]
    lock = threading.Lock()
    stop = threading.Event()
    clients = []

    def download(client):
        try:
            while not stop.is_set():
                data = client.recv(1 << 18)
                if not data:
                    return
                with lock:
                    counted[0] += len(data)
        except OSError:
            pass

    def upload(client):
        try:
            while not stop.is_set():
                client.sendall(ZEROS)
        except OSError:
            pass

    for _ in range(flows):
        port = endpoints.source_port if direction == "download" else endpoints.sink_port
        client = socks_connect(socks_port, port)
        clients.append(client)
        threading.Thread(target=download if direction == "download" else upload, args=(client,), daemon=True).start()

    def total():
        if direction == "download":
            with lock:
                return counted[0]
        with endpoints.lock:
            return endpoints.received

    time.sleep(warmup)
    if voice:
        voice.window()
    before, started = total(), time.monotonic()
    time.sleep(seconds)
    elapsed = time.monotonic() - started
    amount = total() - before
    stop.set()
    for client in clients:
        try:
            client.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        client.close()
    time.sleep(0.5)
    return amount, elapsed


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe", required=True, help="tunnel-core check_route example binary")
    parser.add_argument("--ssh-port", type=int, required=True)
    parser.add_argument("--user", required=True)
    parser.add_argument("--key", required=True, help="throwaway client key path")
    parser.add_argument("--fingerprint", required=True, help="SHA256:... of the fixture host key")
    parser.add_argument("--socks-port", type=int, default=1082)
    parser.add_argument("--one-way-ms", default="0,25,50,100")
    parser.add_argument("--mbit", type=float, default=0, help="rate limit of the link in each direction; 0 is unlimited")
    parser.add_argument("--ssh-window-kib", type=int, default=0,
                        help="client SSH channel receive window; 0 keeps the default (2048)")
    parser.add_argument("--voice", action="store_true",
                        help="run a 160-byte, 50/s UDP echo stream from 3 s before the load and report its round trips")
    parser.add_argument("--dns-lookups", type=int, default=0,
                        help="during each measurement resolve this many names at once every 2 s (real queries to 1.1.1.1)")
    parser.add_argument("--flows", default="1,4")
    parser.add_argument("--directions", default="download,upload")
    parser.add_argument("--seconds", type=float, default=6)
    parser.add_argument("--warmup", type=float, default=3)
    parser.add_argument("--direct", action="store_true", help="skip the delay proxy (CPU ceiling run)")
    parser.add_argument("--cpu-pids", default="", help="PID of the fixture's master sshd, for CPU accounting of its sessions")
    parser.add_argument("--output")
    args = parser.parse_args()

    endpoints = Endpoints()
    proxy = None if args.direct else DelayProxy("127.0.0.1", args.ssh_port)
    delays = [0] if args.direct else [int(value) for value in args.one_way_ms.split(",")]
    rows = []
    for delay_ms in delays:
        if proxy:
            proxy.delay = delay_ms / 1000
            proxy.rate = args.mbit * 1e6 / 8
        config = {
            "server": {
                "name": "fixture", "host": "127.0.0.1",
                "port": args.ssh_port if args.direct else proxy.port,
                "user": args.user, "key_path": args.key, "host_key_sha256": args.fingerprint,
            },
            "socks_port": args.socks_port,
            "seconds": 600,
        }
        if args.ssh_window_kib:
            config["ssh_tuning"] = {"channel_receive_window_bytes": args.ssh_window_kib * 1024}
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as handle:
            json.dump(config, handle)
        probe = subprocess.Popen([args.probe, handle.name, "--interactive"], stdin=subprocess.PIPE,
                                 stdout=subprocess.PIPE, text=True)
        try:
            ready = probe.stdout.readline()
            assert ready.startswith("READY"), ready
            # The probe prints one SAMPLE line per second; keep its pipe drained.
            threading.Thread(target=lambda: [None for _ in probe.stdout], daemon=True).start()
            for direction in args.directions.split(","):
                for flows in [int(value) for value in args.flows.split(",")]:
                    agents = subprocess.run(["pgrep", "-f", "secret-tunnel-agent --stdio-v3"],
                                            capture_output=True, text=True).stdout.split()
                    # sshd session processes are descendants of the fixture's master sshd.
                    sshd, frontier = [], [pid for pid in args.cpu_pids.split(",") if pid]
                    while frontier:
                        children = subprocess.run(["pgrep", "-P", frontier.pop()],
                                                  capture_output=True, text=True).stdout.split()
                        sshd += [pid for pid in children if pid not in agents]
                        frontier += children
                    pids = {"client": [probe.pid], "agent": agents, "sshd": sshd}
                    cpu_before = {name: cpu_seconds(group) for name, group in pids.items()}
                    if proxy:
                        proxy.max_wait = 0.0
                    stop_dns, answered = threading.Event(), []
                    if args.dns_lookups:
                        threading.Thread(target=dns_bursts, daemon=True,
                                         args=(args.socks_port, args.dns_lookups, stop_dns, answered)).start()
                    voice = Voice(args.socks_port) if args.voice else None
                    if voice:
                        time.sleep(3)
                    amount, elapsed = measure(args.socks_port, endpoints, flows, direction,
                                              args.warmup, args.seconds, voice)
                    stop_dns.set()
                    cpu = {name: round(cpu_seconds(group) - cpu_before[name], 2) for name, group in pids.items()}
                    row = {
                        "rtt_ms": delay_ms * 2, "direction": direction, "flows": flows,
                        "mbit_s": round(amount * 8 / 1e6 / elapsed, 2),
                        "law_64k_mbit_s": round(65536 * 8 / 1e6 / (delay_ms * 2 / 1000) * flows, 2) if delay_ms else None,
                        "cpu_seconds_incl_warmup": cpu,
                    }
                    if voice:
                        time.sleep(0.5)
                        row["voice"] = voice.result()
                        voice.close()
                    if args.dns_lookups:
                        row["dns_lookups_per_burst"] = args.dns_lookups
                        row["dns_answered"] = f"{sum(answered)}/{len(answered)}"
                    if proxy and args.mbit:
                        row["link_mbit_s"] = args.mbit
                        row["max_link_queue_ms"] = round(proxy.max_wait * 1000, 1)
                    rows.append(row)
                    print(json.dumps(row), flush=True)
        finally:
            probe.stdin.close()
            try:
                probe.wait(10)
            except subprocess.TimeoutExpired:
                probe.kill()
            os.unlink(handle.name)
    if args.output:
        with open(args.output, "w") as handle:
            json.dump(rows, handle, indent=1)


if __name__ == "__main__":
    main()
