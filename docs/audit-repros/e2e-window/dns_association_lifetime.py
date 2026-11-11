"""How long a SOCKS UDP association lives after its last datagram.

Sends one datagram through the tunnel's local SOCKS listener and waits for the
proxy to close the association's control connection:

  python3 dns_association_lifetime.py SOCKS_PORT dns        # real query to 1.1.1.1:53
  python3 dns_association_lifetime.py SOCKS_PORT echo PORT  # loopback UDP echo, not port 53

A DNS association should close about 5 s after the reply. Any other association
stays open until the client closes it (the script gives up after 12 s).
"""
import socket
import struct
import sys
import time

QUERY = (b"\x12\x34\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00"
         b"\x07example\x03com\x00\x00\x01\x00\x01")


def main():
    socks_port, mode = int(sys.argv[1]), sys.argv[2]
    target = ("1.1.1.1", 53) if mode == "dns" else ("127.0.0.1", int(sys.argv[3]))
    control = socket.create_connection(("127.0.0.1", socks_port), timeout=10)
    control.sendall(b"\x05\x01\x00")
    assert control.recv(2) == b"\x05\x00"
    control.sendall(b"\x05\x03\x00\x01\x00\x00\x00\x00\x00\x00")
    reply = b""
    while len(reply) < 10:
        reply += control.recv(10 - len(reply))
    assert reply[1] == 0, f"UDP ASSOCIATE refused: {reply[1]}"
    relay = (socket.inet_ntoa(reply[4:8]), struct.unpack("!H", reply[8:10])[0])
    udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp.settimeout(5)
    header = b"\x00\x00\x00\x01" + socket.inet_aton(target[0]) + struct.pack("!H", target[1])
    sent = time.monotonic()
    # benchmarks/ssh-transport/echo-server.py echoes only 160-byte datagrams.
    udp.sendto(header + (QUERY if mode == "dns" else bytes(160)), relay)
    data, _ = udp.recvfrom(2048)
    answered = time.monotonic()
    print(f"reply after {(answered - sent) * 1000:.1f} ms, {len(data) - 10} payload bytes")
    control.settimeout(12)
    try:
        closed = control.recv(1) == b""
    except socket.timeout:
        closed = False
    waited = time.monotonic() - answered
    print(f"association {'closed by the proxy' if closed else 'still open'} {waited:.1f} s after the reply")


if __name__ == "__main__":
    main()
