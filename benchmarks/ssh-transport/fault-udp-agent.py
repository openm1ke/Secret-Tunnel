#!/usr/bin/env python3
"""Disposable SSH-fixture agent wrapper: abort one UDP mapping, keep SSH alive.

Only a numeric documentation destination is affected. Used to verify that the
Android TUN adapter recreates an association while the app keeps its UDP socket.
The real receiver-exit handler is covered separately by the agent Rust tests.
"""
import argparse
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import threading


def exact(stream, size):
    data = bytearray()
    while len(data) < size:
        part = stream.read(size - len(data))
        if not part:
            raise EOFError
        data.extend(part)
    return bytes(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", required=True)
    parser.add_argument("--after", type=int, default=25)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("modes", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    modes = args.modes[1:] if args.modes[:1] == ["--"] else args.modes
    if not modes or not modes[0].startswith("--stdio-v3"):
        os.execv(args.backend, [args.backend, *modes])
    if args.after < 1:
        parser.error("--after must be positive")
    mode = modes[0]
    extended = mode not in ("--stdio-v3", "--stdio-v3-diagnostics")
    preface = 16 if mode in ("--stdio-v3-qos", "--stdio-v3-udp-config") else 0
    child = subprocess.Popen([args.backend, *modes], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    write_lock, state_lock = threading.Lock(), threading.Lock()
    udp = set()
    state = {"chosen": None, "replies": 0, "injected": False}

    def to_backend(data):
        with write_lock:
            if not child.stdin.closed:
                child.stdin.write(data)
                child.stdin.flush()

    def frames(stream):
        while True:
            header = exact(stream, 7)
            length, kind, flow = struct.unpack("!HBI", header)
            if not 5 <= length <= 65512:
                raise ValueError("Invalid STMUX003 frame")
            data = exact(stream, length - 5)
            yield header + data, kind, flow, data

    def outgoing():
        try:
            # The agent needs settings before it sends its hello; the client
            # sends its own hello only after reading the agent's.
            if preface:
                to_backend(exact(sys.stdin.buffer, preface))
            to_backend(exact(sys.stdin.buffer, 28 if extended else 20))
            for frame, kind, flow, data in frames(sys.stdin.buffer):
                with state_lock:
                    if kind == 2:
                        udp.add(flow)
                    if (kind == 5 and flow in udp and state["chosen"] is None
                            and data[:10] == b"\0\0\0\1\xcb\0\x71\x0a" + struct.pack("!H", 27150)):
                        state["chosen"] = flow
                    retired = state["injected"] and flow == state["chosen"]
                if not retired:
                    to_backend(frame)
        except (EOFError, BrokenPipeError):
            pass
        finally:
            with write_lock:
                child.stdin.close()

    thread = threading.Thread(target=outgoing, daemon=True)
    thread.start()
    try:
        sys.stdout.buffer.write(exact(child.stdout, 28 if extended else 20))
        sys.stdout.buffer.flush()
        for frame, kind, flow, data in frames(child.stdout):
            with state_lock:
                chosen = flow == state["chosen"]
                if kind == 5 and chosen:
                    state["replies"] += 1
                inject = kind == 5 and chosen and not state["injected"] and state["replies"] >= args.after
                if inject:
                    state["injected"] = True
                    with args.log.open("a") as log:
                        log.write(json.dumps({**state, "agent_pid": child.pid}) + "\n")
                retired = chosen and state["injected"]
            if inject:
                abort = struct.pack("!HBI", 5, 9, flow)
                to_backend(abort)
                sys.stdout.buffer.write(abort)
            elif not retired:
                sys.stdout.buffer.write(frame)
            sys.stdout.buffer.flush()
    except (EOFError, BrokenPipeError):
        pass
    finally:
        if child.poll() is None:
            child.terminate()
        child.wait(timeout=5)


if __name__ == "__main__":
    main()
