"""Check that the impairment bridge preserves bytes and exposes upstream RST."""
import socket
import struct
import threading
import time
import unittest
from matrix import Link


class BridgeTest(unittest.TestCase):
    def test_round_trip_delay_and_half_close_preserve_bytes(self):
        with socket.socket() as server:
            server.bind(("127.0.0.1", 0)); server.listen()
            def echo():
                peer, _ = server.accept()
                with peer:
                    data = b""
                    while part := peer.recv(4096):
                        data += part
                    peer.sendall(data)
            worker = threading.Thread(target=echo); worker.start()
            link = Link("127.0.0.1", server.getsockname()[1], 50, 5)
            try:
                with socket.create_connection(("127.0.0.1", link.port), timeout=3) as client:
                    started = time.monotonic()
                    payload = bytes(range(256)) * 32
                    client.sendall(payload); client.shutdown(socket.SHUT_WR)
                    received = b""
                    while part := client.recv(4096):
                        received += part
                    self.assertEqual(received, payload)
                    self.assertGreaterEqual(time.monotonic()-started, .09)
                self.assertEqual(link.errors, [])
            finally:
                link.close(); worker.join(3)

    def test_upstream_reset_does_not_leave_a_silent_open_tunnel(self):
        with socket.socket() as server:
            server.bind(("127.0.0.1", 0)); server.listen()
            def reset():
                peer, _ = server.accept()
                peer.recv(1)
                peer.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("hh" if __import__('os').name=='nt' else "ii", 1, 0))
                peer.close()
            worker = threading.Thread(target=reset); worker.start()
            link = Link("127.0.0.1", server.getsockname()[1], 0, 0)
            try:
                with socket.create_connection(("127.0.0.1", link.port), timeout=2) as client:
                    client.sendall(b"x")
                    self.assertEqual(client.recv(1), b"")
                self.assertTrue(link.errors)
            finally:
                link.close(); worker.join(3)


if __name__ == "__main__":
    unittest.main()
