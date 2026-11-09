"""Run the phone probe's bounded HTTPS/DNS workload through a SOCKS5 listener."""
import argparse
import concurrent.futures
import json
import math
import os
import socket
import struct
import subprocess
import time


BYTES = 4 * 1024 * 1024
DNS = b"\x00\x00\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x07example\x03com\x00\x00\x01\x00\x01"
UDP_HEADER = b"\x00\x00\x00\x01" + socket.inet_aton("1.1.1.1") + struct.pack("!H", 53)


def exact(sock, size):
    result = bytearray()
    while len(result) < size:
        data = sock.recv(size - len(result))
        if not data:
            raise EOFError("SOCKS control connection closed")
        result.extend(data)
    return bytes(result)


def udp(proxy, keep_going=lambda: True):
    rtts, sent = [], 0
    with socket.create_connection(proxy, timeout=5) as control:
        control.sendall(b"\x05\x01\x00")
        assert exact(control, 2) == b"\x05\x00"
        control.sendall(b"\x05\x03\x00\x01\x00\x00\x00\x00\x00\x00")
        header = exact(control, 4)
        assert header == b"\x05\x00\x00\x01", f"UDP ASSOCIATE failed: {header.hex()}"
        relay = socket.inet_ntoa(exact(control, 4)), struct.unpack("!H", exact(control, 2))[0]
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as datagrams:
            datagrams.connect(relay)
            datagrams.settimeout(1.5)
            for i in range(48):
                if not keep_going():
                    break
                identifier = struct.pack("!H", 0x4100 + i)
                before = time.perf_counter()
                datagrams.send(UDP_HEADER + identifier + DNS[2:])
                sent += 1
                try:
                    data = datagrams.recv(4096)
                    if data.startswith(UDP_HEADER) and len(data) >= len(UDP_HEADER) + 12:
                        response = data[len(UDP_HEADER):]
                        if response[:2] == identifier and response[2] & 128 and not response[3] & 15:
                            rtts.append((time.perf_counter() - before) * 1000)
                except TimeoutError:
                    pass
                time.sleep(.020)
    report = {"sent": sent, "received": len(rtts)}
    if sent:
        report["loss_percent"] = 100 * (sent - len(rtts)) / sent
    if len(rtts) > 1:
        report["mean_rtt_delta_ms"] = sum(abs(b - a) for a, b in zip(rtts, rtts[1:])) / (len(rtts) - 1)
    ordered = sorted(rtts)
    if ordered:
        for p in (50, 95, 99):
            report[f"p{p}_ms"] = ordered[math.ceil(p / 100 * len(ordered)) - 1]
    return report


def download(proxy):
    try:
        result = subprocess.run([
            "curl", "--proxy", f"socks5h://{proxy[0]}:{proxy[1]}", "--noproxy", "",
            "--fail", "--silent", "--show-error", "--max-time", "30", "--output", os.devnull,
            "--write-out", "%{size_download} %{http_code}",
            f"https://speed.cloudflare.com/__down?bytes={BYTES}",
        ], capture_output=True, text=True, timeout=35)
        ok = result.returncode == 0 and result.stdout.strip() == f"{BYTES} 200"
    except subprocess.TimeoutExpired:
        ok = False
    return BYTES if ok else 0, time.perf_counter()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=1080)
    args = parser.parse_args()
    proxy = "127.0.0.1", args.port
    report = {"udp_idle": udp(proxy)}
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as workers:
        start = time.perf_counter()
        downloads = [workers.submit(download, proxy) for _ in range(4)]
        report["udp_bulk"] = udp(proxy, lambda: any(not task.done() for task in downloads))
        data = [task.result() for task in downloads]
    total = sum(count for count, _ in data)
    seconds = max(end for _, end in data) - start
    report.update(tcp_bytes=total, tcp_seconds=seconds, tcp_mbps=total * 8 / seconds / 1e6,
                  tcp_complete=sum(count == BYTES for count, _ in data))
    print(json.dumps(report, indent=2))
    if report["tcp_complete"] != 4 or not report["udp_idle"]["received"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
