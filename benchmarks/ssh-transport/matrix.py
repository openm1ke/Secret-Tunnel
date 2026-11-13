"""Compare actual shared SSH transport with repeatable first-link impairments.

Only the physical entry socket is proxied. Nested SSH, pins and keys remain
unchanged. This models byte-stream delay/rate, not IP packet loss or real LTE.
"""
import argparse
import asyncio
import concurrent.futures
import copy
import hashlib
import ipaddress
import json
import math
import random
import socket
import struct
import subprocess
import threading
import time
from pathlib import Path

SCENARIOS = {"unshaped": (0, 0), "mobile": (50, 5), "long": (150, 2)}
BLOCK = bytes(range(256)) * 64
TCP_BYTES = 2 * 1024 * 1024


class Link:
    """Pipeline propagation delay with bounded buffering and paced writes."""
    def __init__(self, host, port, delay_ms, mbps):
        self.host, self.port_target = host, port
        self.delay = delay_ms / 1000
        self.rate = mbps * 1e6 / 8
        self.loop = asyncio.new_event_loop()
        self.thread = threading.Thread(target=self.loop.run_forever, daemon=True)
        self.thread.start()
        self.connections = set()
        self.errors = []
        self.server = asyncio.run_coroutine_threadsafe(self.start(), self.loop).result(5)
        self.port = self.server.sockets[0].getsockname()[1]

    async def start(self):
        return await asyncio.start_server(self.accept, "127.0.0.1", 0, limit=16384)

    async def pipe(self, reader, writer):
        queue = asyncio.Queue(maxsize=8)  # <=64 KiB in the explicit delay queue.

        async def read():
            try:
                while True:
                    data = await reader.read(8192)
                    await queue.put((time.monotonic() + self.delay, data))
                    if not data:
                        return
            except Exception as error:
                # Propagate a reset instead of leaving the consumer waiting on
                # an empty queue with a still-open local TCP connection.
                await queue.put((time.monotonic(), error))

        producer = asyncio.create_task(read())
        available = time.monotonic()
        try:
            while True:
                due, data = await queue.get()
                if isinstance(data, Exception):
                    raise data
                await asyncio.sleep(max(0, max(due, available) - time.monotonic()))
                if not data:
                    if writer.can_write_eof():
                        writer.write_eof()
                    return
                writer.write(data)
                await writer.drain()
                available = max(time.monotonic(), available) + (len(data) / self.rate if self.rate else 0)
        finally:
            producer.cancel()
            await asyncio.gather(producer, return_exceptions=True)

    async def accept(self, reader, writer):
        task = asyncio.current_task()
        self.connections.add(task)
        upstream = None
        pipes = []
        try:
            incoming, upstream = await asyncio.open_connection(self.host, self.port_target, limit=16384)
            pipes = [asyncio.create_task(self.pipe(reader, upstream)), asyncio.create_task(self.pipe(incoming, writer))]
            await asyncio.gather(*pipes)
        except OSError as error:
            self.errors.append(str(error))
        except asyncio.CancelledError:
            pass
        finally:
            for pipe in pipes:
                pipe.cancel()
            await asyncio.gather(*pipes, return_exceptions=True)
            for stream in (writer, upstream):
                if stream:
                    stream.close()
            self.connections.discard(task)

    async def shutdown(self):
        self.server.close()
        await self.server.wait_closed()
        tasks = list(self.connections)
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)

    def close(self):
        asyncio.run_coroutine_threadsafe(self.shutdown(), self.loop).result(5)
        self.loop.call_soon_threadsafe(self.loop.stop)
        self.thread.join(5)
        self.loop.close()


def exact(stream, size):
    data = b""
    while len(data) < size:
        part = stream.recv(size - len(data))
        if not part:
            raise RuntimeError("Truncated SOCKS response")
        data += part
    return data


def target(host, port):
    address = ipaddress.ip_address(host)
    return bytes([1 if address.version == 4 else 4]) + address.packed + struct.pack("!H", port)


def socks(port, command, destination):
    stream = socket.create_connection(("127.0.0.1", port), timeout=45)
    stream.sendall(b"\x05\x01\x00")
    assert exact(stream, 2) == b"\x05\x00"
    stream.sendall(bytes([5, command, 0]) + destination)
    header = exact(stream, 4)
    assert header[:3] == b"\x05\x00\x00", header
    size = {1: 4, 4: 16}[header[3]]
    address = socket.inet_ntop(socket.AF_INET if size == 4 else socket.AF_INET6, exact(stream, size))
    return stream, (address, struct.unpack("!H", exact(stream, 2))[0])


def tcp_load(port, host, remote_port, expected_bytes):
    started = time.monotonic()
    stream, _ = socks(port, 1, target(host, remote_port))
    digest = hashlib.sha256()
    received = 0
    with stream:
        stream.sendall(b"PAYLOAD\n" if expected_bytes == TCP_BYTES else f"PAYLOAD {expected_bytes}\n".encode())
        while data := stream.recv(65536):
            received += len(data)
            digest.update(data)
    assert received == expected_bytes, f"Truncated TCP body: {received}/{expected_bytes} bytes after {time.monotonic()-started:.2f}s"
    assert digest.digest() == hashlib.sha256(BLOCK * (expected_bytes // len(BLOCK))).digest()
    finished = time.monotonic()
    return {"bytes": received, "seconds": finished - started, "finished_monotonic": finished}


def voice(port, host, remote_port, duration, load_start, *, size=160, packets_per_second=50, start_at=None):
    control, relay = socks(port, 3, bytes([1]) + bytes(6))
    header = bytes(3) + target(host, remote_port)
    sent, received = {}, {}
    errors = []
    with control, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
        udp.connect(relay)
        udp.settimeout(.1)
        start = start_at if start_at is not None else time.monotonic()
        if start_at is not None and start <= time.monotonic():
            raise RuntimeError("UDP association was not ready before the workload")
        deadline = start + duration + 3

        def receive():
            try:
                while time.monotonic() < deadline:
                    try:
                        packet = udp.recv(65535)
                    except socket.timeout:
                        continue
                    assert packet[:len(header)] == header and len(packet) == len(header) + size
                    sequence = struct.unpack("!I", packet[len(header):len(header) + 4])[0]
                    assert packet[len(header) + 4:] == bytes(size - 4)
                    now = time.monotonic()
                    if sequence in sent and sequence not in received:
                        received[sequence] = 1000 * (now - sent[sequence])
            except BaseException as error:
                errors.append(str(error))

        receiver = threading.Thread(target=receive)
        receiver.start()
        for sequence in range(duration * packets_per_second):
            time.sleep(max(0, start + sequence / packets_per_second - time.monotonic()))
            sent[sequence] = time.monotonic()
            udp.send(header + struct.pack("!I", sequence) + bytes(size - 4))
        receiver.join()
    assert not errors, errors
    values = sorted(received.values())
    return {"sent": len(sent), "received": len(received), "within_700ms": sum(v <= 700 for v in values),
            "p95_all_received_ms": values[math.ceil(.95 * len(values)) - 1] if values else None,
            "max_ms": max(values) if values else None,
            "packets": [{"sent_ms": 1000 * (when - load_start), "rtt_ms": received.get(sequence)} for sequence, when in sent.items()]}


def run_one(args, base, route, scenario, queue, repetition):
    config = copy.deepcopy(base)
    if route == "direct":
        config["jumps"] = []
    entry = config["jumps"][0] if config.get("jumps") else config["server"]
    delay_ms, mbps = SCENARIOS[scenario]
    link = Link(entry["host"], entry["port"], delay_ms, mbps)
    entry.update(host="127.0.0.1", port=link.port)
    config["ssh_tuning"] = {"cipher": "auto", "channel_buffer_messages": queue, "channel_receive_window_bytes": 2097152}
    config["ssh_tuning"]["tcp_interactive_bytes"] = args.tcp_budget_kib * 1024 if args.tcp_budget_kib else None
    config["socks_port"] = args.socks_port
    config["seconds"] = 100
    name = f"{repetition}-{route}-{scenario}-s{args.tcp_streams}-q{queue}"
    config_file = args.output / (name + ".config.json")
    config_file.write_text(json.dumps(config), encoding="utf-8")
    trace_file = args.output / (name + ".trace")
    flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    process = None
    try:
        with trace_file.open("w", encoding="utf-8") as output:
            process = subprocess.Popen([str(args.probe), str(config_file), "--interactive"], stdin=subprocess.PIPE, stdout=output, stderr=subprocess.STDOUT, creationflags=flags)
            deadline = time.monotonic() + 30
            while "READY " not in trace_file.read_text(encoding="utf-8"):
                assert process.poll() is None, trace_file.read_text(encoding="utf-8")
                assert time.monotonic() < deadline, "SSH startup timed out"
                time.sleep(.2)
            # Include an idle probe before bulk. All loaded probes share SSH.
            time.sleep(1.5)
            started = time.time()
            load_start = time.monotonic()
            with concurrent.futures.ThreadPoolExecutor(max_workers=args.tcp_streams + 1) as pool:
                udp = pool.submit(voice, args.socks_port, base["targets"].get("host", "127.0.0.1"), base["targets"]["udp_port"], args.duration + args.voice_lead_seconds, load_start)
                if args.voice_lead_seconds: time.sleep(args.voice_lead_seconds)
                bulk_start_ms = 1000 * (time.monotonic() - load_start)
                bulk = [pool.submit(tcp_load, args.socks_port, base["targets"].get("host", "127.0.0.1"), base["targets"]["tcp_port"], 2 * TCP_BYTES // args.tcp_streams) for _ in range(args.tcp_streams)]
                result = {"route": route, "scenario": scenario, "queue": queue, "window_bytes": 2097152, "cipher": "auto",
                          "repetition": repetition, "tcp_streams": args.tcp_streams, "tcp_budget_kib":args.tcp_budget_kib, "voice_lead_seconds":args.voice_lead_seconds, "added_one_way_delay_ms": delay_ms, "per_direction_mbps": mbps or None,
                          "load_started_unix": started, "tcp": [item.result() for item in bulk], "voice": udp.result()}
            bulk_end_ms = 1000 * (max(item.pop("finished_monotonic") for item in result["tcp"]) - load_start)
            loaded = [packet for packet in result["voice"]["packets"] if bulk_start_ms <= packet["sent_ms"] <= bulk_end_ms]
            values = sorted(packet["rtt_ms"] for packet in loaded if packet["rtt_ms"] is not None)
            result["voice_during_tcp"] = {"sent": len(loaded), "received": len(values), "within_700ms": sum(value <= 700 for value in values),
                                          "p95_all_received_ms": values[math.ceil(.95 * len(values)) - 1] if values else None}
            time.sleep(1.5)
            samples = [json.loads(row.split(" ", 1)[1]) for row in trace_file.read_text(encoding="utf-8").splitlines() if row.startswith("SAMPLE ")]
            assert process.poll() is None, "Transport exited while measuring"
            assert samples and samples[-1]["client"]["heartbeat"]["acknowledged"] > 0, "No end-to-end heartbeat"
            result["samples"] = samples
            result["bridge_errors"] = list(link.errors)
            assert not link.errors, link.errors
            result["tcp_mbps"] = 8 * sum(row["bytes"] for row in result["tcp"]) / max(row["seconds"] for row in result["tcp"]) / 1e6
            (args.output / (name + ".json")).write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
            print(json.dumps({key: result[key] for key in ("route", "scenario", "queue", "repetition", "tcp_mbps", "voice_during_tcp")}), flush=True)
    except Exception as error:
        (args.output / (name + ".failure.json")).write_text(json.dumps({"route":route,"scenario":scenario,"queue":queue,"repetition":repetition,"tcp_streams":args.tcp_streams,"error":str(error),"bridge_errors":list(link.errors)},indent=2)+'\n',encoding='utf-8')
        raise
    finally:
        # Let check_route finish normally, so nested SSH sessions send a clean
        # disconnect. No other client's process or profile is touched.
        if process and process.poll() is None:
            process.stdin.close()
            try:
                process.wait(timeout=8)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.wait(timeout=8)
                raise RuntimeError("Probe did not stop gracefully")
        link.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--routes", default="direct,chain")
    parser.add_argument("--scenarios", default="unshaped,mobile,long")
    parser.add_argument("--queues", default="32,8")
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--first-repeat", type=int, default=0)
    parser.add_argument("--duration", type=int, default=12)
    parser.add_argument("--socks-port", type=int, default=1082)
    parser.add_argument("--tcp-streams", type=int, default=2)
    parser.add_argument("--tcp-budget-kib", type=int, default=0)
    parser.add_argument("--voice-lead-seconds",type=int,default=0)
    args = parser.parse_args()
    base = json.loads(args.config.read_text(encoding="utf-8-sig"))
    assert all(base["targets"][key] > 0 for key in ("tcp_port", "udp_port"))
    assert 1 <= args.repeats <= 10 and 3 <= args.duration <= 60
    assert args.tcp_streams in (2, 4, 8, 16)
    assert args.tcp_budget_kib == 0 or 64 <= args.tcp_budget_kib <= 8192
    assert 0 <= args.voice_lead_seconds <= 10
    routes, scenarios, queues = args.routes.split(","), args.scenarios.split(","), list(map(int, args.queues.split(",")))
    assert set(routes) <= {"direct", "chain"} and set(scenarios) <= SCENARIOS.keys()
    assert set(queues) <= {8, 16, 32, 64}
    if "chain" in routes:
        assert base.get("jumps"), "Chain measurements need at least one jump"
    args.output.mkdir(parents=True, exist_ok=True)
    args.probe = args.probe.resolve()
    for repetition in range(args.first_repeat, args.first_repeat + args.repeats):
        # Adjacent pairs share the same route/impairment; reverse queue order
        # between repeats rather than assigning one queue to an earlier period.
        pairs = [(route, scenario) for route in routes for scenario in scenarios]
        random.Random(20261110 + repetition).shuffle(pairs)
        for route, scenario in pairs:
            for queue in queues if repetition % 2 == 0 else reversed(queues):
                run_one(args, base, route, scenario, queue, repetition)


if __name__ == "__main__":
    main()
