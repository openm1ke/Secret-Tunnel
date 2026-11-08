"""Exercise the real agent binary, framing, UDP responses and clean EOF.

Run: python udp-agent/test_echo.py udp-agent/target/debug/secret-tunnel-agent.exe
"""
import concurrent.futures
import socket
import struct
import subprocess
import sys


def main():
    # macOS defaults net.inet.udp.maxdgram to 9216; larger sendto calls fail
    # with EMSGSIZE even on loopback. Keep the large-payload test within that
    # system limit without changing a machine-wide sysctl for the test.
    large_payload = 8192 if sys.platform == "darwin" else 51200
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as echo:
        echo.bind(("127.0.0.1", 0))
        echo.settimeout(10)
        with subprocess.Popen([sys.argv[1]], stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE) as agent:
            def exercise():
                assert agent.stdout.read(8) == b"STUDP001"
                for payload in (b"", b"voice", bytes(range(256)) * (large_payload // 256)):
                    packet = b"\0\0\0\1" + socket.inet_aton("127.0.0.1")
                    packet += struct.pack("!H", echo.getsockname()[1]) + payload
                    agent.stdin.write(struct.pack("!H", len(packet)) + packet)
                    agent.stdin.flush()
                    data, source = echo.recvfrom(65535)
                    assert data == payload
                    echo.sendto(data, source)
                    length = struct.unpack("!H", agent.stdout.read(2))[0]
                    reply = agent.stdout.read(length)
                    assert reply == packet
                agent.stdin.close()
                assert agent.wait(timeout=5) == 0, agent.stderr.read()

            with concurrent.futures.ThreadPoolExecutor() as pool:
                future = pool.submit(exercise)
                try:
                    future.result(timeout=25)
                finally:
                    if agent.poll() is None:
                        agent.kill()
            print(f"PASS: empty, voice and {large_payload}-byte UDP payloads; framing; clean EOF")

    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as echo:
        echo.bind(("127.0.0.1", 0)); echo.settimeout(10)
        with subprocess.Popen([sys.argv[1], "--multiplex"], stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE) as agent:
            def multiplex():
                assert agent.stdout.read(8) == b"STUDP002"
                destination = b"\0\0\0\1" + socket.inet_aton("127.0.0.1") + struct.pack("!H", echo.getsockname()[1])
                def send(association, payload):
                    body = struct.pack("!I", association) + (destination + payload if payload is not None else b"")
                    agent.stdin.write(struct.pack("!H", len(body)) + body); agent.stdin.flush()
                for association in range(1, 25):
                    send(association, struct.pack("!I", association) + b"media")
                requests = [echo.recvfrom(4096) for _ in range(24)]
                assert len({source for _, source in requests}) == 24, "UDP sources were merged"
                for data, source in reversed(requests): echo.sendto(data, source)
                replies = set()
                for _ in range(24):
                    length = struct.unpack("!H", agent.stdout.read(2))[0]; body = agent.stdout.read(length)
                    association = struct.unpack("!I", body[:4])[0]
                    assert body[4:] == destination + struct.pack("!I", association) + b"media"
                    replies.add(association)
                assert replies == set(range(1, 25)), "Association responses crossed"
                send(7, None)
                send(8, b"still alive")
                data, source = echo.recvfrom(4096); assert data == b"still alive"; echo.sendto(data, source)
                length = struct.unpack("!H", agent.stdout.read(2))[0]; body = agent.stdout.read(length)
                assert struct.unpack("!I", body[:4])[0] == 8
                agent.stdin.close(); assert agent.wait(timeout=5) == 0, agent.stderr.read()
            with concurrent.futures.ThreadPoolExecutor() as pool:
                future = pool.submit(multiplex)
                try: future.result(timeout=25)
                finally:
                    if agent.poll() is None: agent.kill()
            print("PASS: 24 multiplexed UDP associations, distinct source ports, reversed responses and isolated close")


if __name__ == "__main__":
    main()
