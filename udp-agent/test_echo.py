"""Exercise the real agent binary, framing, UDP responses and clean EOF.

Run: python udp-agent/test_echo.py udp-agent/target/debug/secret-tunnel-agent.exe
"""
import concurrent.futures
import socket
import struct
import subprocess
import sys


def main():
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as echo:
        echo.bind(("127.0.0.1", 0))
        echo.settimeout(10)
        with subprocess.Popen([sys.argv[1]], stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE) as agent:
            def exercise():
                assert agent.stdout.read(8) == b"STUDP001"
                for payload in (b"", b"voice", bytes(range(256)) * 200):
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
            print("PASS: empty, voice and 51200-byte UDP payloads; framing; clean EOF")


if __name__ == "__main__":
    main()
