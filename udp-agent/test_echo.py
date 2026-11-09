"""Independent STMUX003 smoke: real TCP half-close and UDP echo backends."""
import concurrent.futures
import socket
import struct
import subprocess
import sys


def exact(stream, count):
    result = bytearray()
    while len(result) < count:
        part = stream.read(count - len(result))
        if not part:
            raise EOFError("Truncated STMUX003 response")
        result.extend(part)
    return bytes(result)


def exercise(agent):
    hello = exact(agent.stdout, 20)
    assert hello[:8] == b"STMUX003"
    agent.stdin.write(b"STMUX003" + struct.pack("!III", 64, 65536, 32))
    agent.stdin.flush()

    def send(kind, flow, data=b""):
        agent.stdin.write(struct.pack("!HBI", len(data) + 5, kind, flow) + data)
        agent.stdin.flush()

    def receive(kind, flow=None):
        while True:
            length = struct.unpack("!H", exact(agent.stdout, 2))[0]
            assert 5 <= length <= 65512
            frame = exact(agent.stdout, length)
            actual_kind, actual_flow = struct.unpack("!BI", frame[:5])
            data = frame[5:]
            if actual_kind == 5:
                send(8, actual_flow, struct.pack("!II", len(data), 1))
            if actual_kind == kind and (flow is None or actual_flow == flow):
                return (actual_flow, data) if flow is None else data
            assert actual_kind in (7, 8, 10), (actual_kind, actual_flow)

    def target(port):
        return b"\0\0\0\1\x7f\0\0\1" + struct.pack("!H", port)

    with socket.socket() as backend:
        backend.bind(("127.0.0.1", 0))
        backend.listen(1)
        backend.settimeout(10)
        send(1, 1, target(backend.getsockname()[1]))
        receive(3, 1)
        with backend.accept()[0] as peer:
            peer.settimeout(10)
            payload = b"TCP reply after request half-close"
            send(5, 1, payload)
            send(6, 1)
            received = b""
            while True:
                chunk = peer.recv(4096)
                if not chunk:
                    break
                received += chunk
            assert received == payload
            peer.sendall(payload)
            peer.shutdown(socket.SHUT_WR)
            reply = b""
            while len(reply) < len(payload):
                reply += receive(5, 1)
            assert reply == payload
            receive(6, 1)
            send(7, 1)

    large = 8192 if sys.platform == "darwin" else 51200
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as echo:
        echo.bind(("127.0.0.1", 0))
        echo.settimeout(10)
        destination = target(echo.getsockname()[1])
        send(2, 3)
        receive(3, 3)
        for payload in (b"", b"voice", bytes(range(256)) * (large // 256)):
            send(5, 3, destination + payload)
            data, source = echo.recvfrom(65535)
            assert data == payload
            echo.sendto(data, source)
            assert receive(5, 3) == destination + payload
        flows = list(range(5, 53, 2))
        for flow in flows:
            send(2, flow)
            receive(3, flow)
            send(5, flow, destination + struct.pack("!I", flow) + b"media")
        requests = [echo.recvfrom(4096) for _ in flows]
        assert len({source for _, source in requests}) == len(flows)
        for data, source in reversed(requests):
            echo.sendto(data, source)
        seen = set()
        for _ in flows:
            flow, data = receive(5)
            assert flow in flows and flow not in seen
            assert data == destination + struct.pack("!I", flow) + b"media"
            seen.add(flow)
        send(9, 7)
        send(5, 9, destination + b"still alive")
        data, source = echo.recvfrom(4096)
        assert data == b"still alive"
        echo.sendto(data, source)
        assert receive(5, 9) == destination + data
    agent.stdin.close()
    assert agent.wait(timeout=5) == 0, agent.stderr.read()
    print(f"PASS STMUX003: TCP half-close; empty/voice/{large}-byte UDP; 24 isolated associations; EOF")


def main():
    with subprocess.Popen([sys.argv[1], "--stdio-v3"], stdin=subprocess.PIPE,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE) as agent:
        with concurrent.futures.ThreadPoolExecutor() as pool:
            task = pool.submit(exercise, agent)
            try:
                task.result(timeout=30)
            finally:
                if agent.poll() is None:
                    agent.kill()


if __name__ == "__main__":
    main()
