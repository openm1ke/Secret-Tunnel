"""Integration check: SOCKS TCP/TLS and UDP DNS through the same backend.

Run against an embedded SOCKS listener: python scripts/check_socks_udp.py 1082
Uses public example.com and Cloudflare DNS; never reads SSH keys.
"""
import concurrent.futures
import os
import socket
import ssl
import struct
import sys
import time


def exact(stream, length):
    data = b""
    while len(data) < length:
        part = stream.recv(length - len(data))
        if not part:
            raise RuntimeError("SOCKS stream closed")
        data += part
    return data


def socks_request(port, command, address):
    stream = socket.create_connection(("127.0.0.1", port), timeout=10)
    stream.sendall(b"\x05\x01\x00")
    assert exact(stream, 2) == b"\x05\x00"
    stream.sendall(b"\x05" + bytes([command]) + b"\x00" + address)
    head = exact(stream, 4)
    assert head[:3] == b"\x05\x00\x00", head
    if head[3] == 1:
        host = socket.inet_ntop(socket.AF_INET, exact(stream, 4))
    elif head[3] == 4:
        host = socket.inet_ntop(socket.AF_INET6, exact(stream, 16))
    else:
        host = exact(stream, exact(stream, 1)[0]).decode()
    bound_port = struct.unpack("!H", exact(stream, 2))[0]
    return stream, (host, bound_port)


def tcp_check(port):
    host = b"example.com"
    raw, _ = socks_request(port, 1, b"\x03" + bytes([len(host)]) + host + struct.pack("!H", 443))
    with ssl.create_default_context().wrap_socket(raw, server_hostname=host.decode()) as stream:
        stream.sendall(b"GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n")
        response = b""
        while True:
            data = stream.recv(65536)
            if not data:
                break
            response += data
        assert response.startswith(b"HTTP/1.1 200"), response[:100]
        print("PASS TCP HTTPS through SOCKS")


def udp_check(port):
    control, relay = socks_request(port, 3, b"\x01\x00\x00\x00\x00\x00\x00")
    with control, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
        udp.bind(("127.0.0.1", 0))
        udp.settimeout(10)
        target = b"\x00\x00\x00\x01" + socket.inet_aton("1.1.1.1") + struct.pack("!H", 53)
        for _ in range(8):
            transaction = os.urandom(2)
            query = transaction + struct.pack("!HHHHH", 0x0100, 1, 0, 0, 0)
            query += b"\x07example\x03com\x00\x00\x01\x00\x01"
            udp.sendto(target + query, relay)
            reply, source = udp.recvfrom(65535)
            assert source == relay
            assert reply[:10] == target
            dns = reply[10:]
            assert dns[:2] == transaction and dns[2] & 0x80
            assert dns[3] & 0x0f == 0, dns[:12]
        print("PASS UDP: 8 DNS responses through SSH agent")
        control.close()
        time.sleep(0.3)
        udp.settimeout(1)
        udp.sendto(target + query, relay)
        try:
            udp.recvfrom(65535)
        except (socket.timeout, ConnectionResetError):
            print("PASS UDP association closes with control TCP")
        else:
            raise AssertionError("UDP relay survived control TCP close")


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 1082
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        futures = [pool.submit(tcp_check, port), pool.submit(udp_check, port)]
        for future in futures:
            future.result(timeout=30)
