#!/usr/bin/env python3
"""Bounded load probe against loopback echo backends reached through SOCKS.

The caller owns the remote TCP/UDP echo listeners. No public service is stressed.
"""
import argparse
import asyncio
import json
import socket
import time


async def exercise(args):
    writers, datagrams = [], []
    setup = asyncio.Semaphore(64)

    async def socks(command, port):
        async with setup:
            reader, writer = await asyncio.open_connection("127.0.0.1", args.socks_port)
            writers.append(writer)
            writer.write(b"\x05\x01\x00")
            await writer.drain()
            assert await reader.readexactly(2) == b"\x05\x00"
            writer.write(bytes([5, command, 0, 1, 127, 0, 0, 1]) + port.to_bytes(2, "big"))
            await writer.drain()
            reply = await asyncio.wait_for(reader.readexactly(10), 15)
            assert reply[0] == 5 and reply[3] == 1
            return reader, writer, reply

    async def echo(connection, index, size=1028):
        reader, writer, reply = connection
        assert reply[1] == 0, f"SOCKS failure {reply[1]}"
        payload = index.to_bytes(4, "big") + bytes([index % 251]) * (size - 4)
        async def upload():
            writer.write(payload)
            await writer.drain()
            writer.write_eof()
        upload_task = asyncio.create_task(upload())
        actual = await asyncio.wait_for(reader.readexactly(size), 30)
        assert actual == payload, "Flow payload crossed or truncated"
        assert await asyncio.wait_for(reader.read(1), 10) == b"", "Missing half-close"
        await upload_task
        writer.close()
        await writer.wait_closed()

    try:
        started = time.monotonic()
        connections = await asyncio.gather(*(socks(1, args.tcp_port) for _ in range(args.count)))
        assert all(c[2][1] == 0 for c in connections)
        extra = await socks(1, args.tcp_port)
        assert extra[2][1] != 0, "Capacity overflow must be rejected explicitly"
        extra[1].close()
        await extra[1].wait_closed()
        if args.hold_seconds:
            print(json.dumps({"stage": "held", "live_tcp": len(connections)}), flush=True)
            await asyncio.sleep(args.hold_seconds)
        await asyncio.gather(*(echo(c, i) for i, c in enumerate(connections)))
        tcp_seconds = time.monotonic() - started
        # Allow ordered close frames to return admission slots on both ends.
        await asyncio.sleep(0.5)

        async def udp(index):
            _, control, reply = await socks(3, 0)
            assert reply[1] == 0
            udp_socket = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            udp_socket.setblocking(False)
            datagrams.append(udp_socket)
            relay = (socket.inet_ntoa(reply[4:8]), int.from_bytes(reply[8:10], "big"))
            header = b"\0\0\0\x01\x7f\0\0\x01" + args.udp_port.to_bytes(2, "big")
            payload = index.to_bytes(4, "big") + bytes([index % 251]) * 4096
            loop = asyncio.get_running_loop()
            for _ in range(4):
                await loop.sock_sendto(udp_socket, header + payload, relay)
                packet, _ = await asyncio.wait_for(loop.sock_recvfrom(udp_socket, 8192), 20)
                assert packet == header + payload, "UDP source/payload crossed"
            control.close()
            await control.wait_closed()

        # Leave a large echo unread while unrelated TCP and UDP must progress.
        slow_reader, slow_writer, reply = await socks(1, args.tcp_port)
        assert reply[1] == 0
        slow_writer.write(b"s" * (4 * 1024 * 1024))
        slow_upload = asyncio.create_task(slow_writer.drain())

        async def latency():
            timings = []
            for index in range(16):
                begin = time.monotonic()
                await echo(await socks(1, args.tcp_port), index)
                timings.append((time.monotonic() - begin) * 1000)
            return max(timings)

        async def bulk():
            await echo(await socks(1, args.tcp_port), 12345, 8 * 1024 * 1024)

        results = await asyncio.gather(latency(), bulk(), *(udp(i) for i in range(64)))
        slow_writer.transport.abort()
        slow_upload.cancel()
        await asyncio.gather(slow_upload, return_exceptions=True)
        return {"passed": True, "tcp_simultaneous": args.count,
                "tcp_payloads_exact": args.count, "capacity_overflow_rejected": True,
                "tcp_phase_seconds": round(tcp_seconds, 2), "udp_associations": 64,
                "udp_replies": 256, "bulk_bytes_exact": 8 * 1024 * 1024,
                "slow_flow_isolated": True, "small_tcp_max_ms": round(results[0], 1)}
    finally:
        for writer in writers:
            writer.transport.abort()
        for udp_socket in datagrams:
            udp_socket.close()
        await asyncio.gather(*(writer.wait_closed() for writer in writers), return_exceptions=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socks-port", type=int, default=1082)
    parser.add_argument("--tcp-port", type=int, required=True)
    parser.add_argument("--udp-port", type=int, required=True)
    parser.add_argument("--count", type=int, default=2048, choices=range(64, 2049))
    parser.add_argument("--hold-seconds", type=int, default=0, choices=range(0, 31))
    arguments = parser.parse_args()
    try:
        import resource
        _, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
        resource.setrlimit(resource.RLIMIT_NOFILE, (min(hard, 8192), hard))
    except ImportError:
        pass
    print(json.dumps(asyncio.run(asyncio.wait_for(exercise(arguments), 180))))
