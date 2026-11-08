"""Bounded end-to-end UDP echo benchmark through an existing SOCKS association.

The destination must be an echo service on the SSH server's loopback interface.
No public UDP port is needed. Example: python scripts/benchmark_udp.py 1082 38494 result.json
"""
import hashlib
import json
import socket
import struct
import sys
import time
from check_socks_udp import socks_request


def percentile(samples, fraction):
    return round(sorted(samples)[min(len(samples)-1, int(len(samples)*fraction))], 3)


def run(proxy_port, echo_port):
    control, relay = socks_request(proxy_port, 3, b"\x01\x00\x00\x00\x00\x00\x00")
    target = b"\0\0\0\1" + socket.inet_aton("127.0.0.1") + struct.pack("!H", echo_port)
    results = {}
    with control, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
        udp.bind(("127.0.0.1", 0))
        udp.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 4*1024*1024)
        udp.settimeout(5)
        for size in (64, 1200, 8192, 60000):
            payload = bytes(index % 251 for index in range(size))
            timings = []
            for _ in range(20):
                start = time.perf_counter()
                udp.sendto(target+payload, relay)
                reply, source = udp.recvfrom(65535)
                assert source == relay and reply[:10] == target
                assert reply[10:] == payload, "Payload mismatch"
                timings.append((time.perf_counter()-start)*1000)
            results[str(size)] = {"packets":20,"integrity":"ok","rtt_p50_ms":percentile(timings,.5),"rtt_p95_ms":percentile(timings,.95)}
        total, window, size = 2000, 64, 1200
        sent, received = {}, set()
        next_id = 0
        start = time.perf_counter()
        deadline = start + 20
        expected_digest = hashlib.sha256(bytes(range(256))*4+b"v"*168).digest()
        while time.perf_counter() < deadline and len(received) < total:
            while next_id < total and len(sent)-len(received) < window:
                body = bytes(range(256))*4+b"v"*168
                udp.sendto(target+struct.pack("!Q", next_id)+body, relay)
                sent[next_id] = time.perf_counter()
                next_id += 1
            udp.settimeout(min(1, max(.01, deadline-time.perf_counter())))
            try:
                reply, source = udp.recvfrom(65535)
            except socket.timeout:
                # Bound the test even if a lost UDP packet consumes a window slot.
                if next_id == total or time.perf_counter()-start > 15:
                    break
                continue
            assert source == relay and reply[:10] == target
            sequence = struct.unpack("!Q", reply[10:18])[0]
            assert sequence in sent and len(reply[10:]) == size
            assert hashlib.sha256(reply[18:]).digest() == expected_digest
            received.add(sequence)
        elapsed = time.perf_counter()-start
        results["pipeline"] = {"sent":len(sent),"received":len(received),"missing":len(sent)-len(received),"payload_bytes":size,
            "seconds":round(elapsed,3),"payload_mbps":round(len(received)*size*8/elapsed/1e6,3),"integrity":"ok"}
    return results


if __name__ == "__main__":
    result = run(int(sys.argv[1]), int(sys.argv[2]))
    with open(sys.argv[3],"w",encoding="utf-8") as output:
        json.dump(result,output,indent=2)
    print(json.dumps(result,indent=2))
