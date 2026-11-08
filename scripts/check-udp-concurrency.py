#!/usr/bin/env python3
"""Probe independent UDP associations through an already-running local SOCKS5."""
import argparse
import asyncio
import json
import socket


async def exercise(port, count):
    controls, sockets = [], []
    async def association(index):
        try:
            reader, writer = await asyncio.open_connection("127.0.0.1", port)
            controls.append(writer)
            writer.write(b"\x05\x01\x00"); await writer.drain()
            assert await reader.readexactly(2) == b"\x05\x00"
            writer.write(b"\x05\x03\x00\x01" + b"\0" * 6); await writer.drain()
            reply = await asyncio.wait_for(reader.readexactly(10), 12)
            assert reply[:4] == b"\x05\x00\x00\x01"
            udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            udp.setblocking(False); sockets.append(udp)
            query = (index + 1).to_bytes(2, "big") + b"\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x07example\x03com\0\0\x01\0\x01"
            packet = b"\0\0\0\x01\x01\x01\x01\x01\x00\x35" + query
            loop = asyncio.get_running_loop()
            await loop.sock_sendto(udp, packet, (socket.inet_ntoa(reply[4:8]), int.from_bytes(reply[8:10], "big")))
            response, _ = await asyncio.wait_for(loop.sock_recvfrom(udp, 4096), 10)
            assert len(response) >= 22 and response[10:12] == query[:2]
            assert response[12] & 128 and response[13] & 15 == 0
            return "ok"
        except Exception as error:
            return type(error).__name__
    try:
        results = await asyncio.gather(*(association(index) for index in range(count)))
        return {"expected": count, "replies": results.count("ok"),
                "outcomes": {key: results.count(key) for key in sorted(set(results))},
                "passed": results.count("ok") == count}
    finally:
        for writer in controls: writer.close()
        for udp in sockets: udp.close()
        await asyncio.gather(*(writer.wait_closed() for writer in controls), return_exceptions=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=1082)
    parser.add_argument("--count", type=int, default=24, choices=range(1, 129))
    arguments = parser.parse_args()
    result = asyncio.run(exercise(arguments.port, arguments.count))
    print(json.dumps(result))
    raise SystemExit(0 if result["passed"] else 1)
