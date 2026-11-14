"""Compare independent SSH connections over one shared, impaired Linux link.

Run as root inside a disposable network namespace. All test traffic stays on
two veth interfaces and loopback, with a temporary sshd and agent binary.
"""
import argparse
import concurrent.futures
import contextlib
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time

from matrix import BLOCK, exact, socks, target, tcp_load, voice

SERVER_IP = "10.229.0.1"
CLIENT_IP = "10.229.0.2"
SSH_PORT = 22002
SCENARIOS = {
    "clean": (50, 0, 0, 5, 96),
    "lossy": (50, 10, 0.3, 5, 96),
    "narrow": (100, 20, 0.3, 2, 72),
}


def command(*args):
    return subprocess.check_output(args, text=True).strip()


@contextlib.contextmanager
def process(args, **kwargs):
    proc = subprocess.Popen(args, **kwargs)
    try:
        yield proc
    finally:
        if proc.stdin:
            proc.stdin.close()
            try:
                proc.wait(timeout=8)
            except subprocess.TimeoutExpired:
                pass
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()


def wait_ready(proc, trace):
    deadline = time.monotonic() + 28
    while "READY " not in trace.read_text():
        if proc.poll() is not None or time.monotonic() > deadline:
            raise RuntimeError(f"SSH probe did not become ready. See {trace}")
        time.sleep(.05)


def tcp_upload(port, remote_port, size):
    started = time.monotonic()
    stream, _ = socks(port, 1, target("127.0.0.1", remote_port))
    digest = hashlib.sha256()
    with stream:
        stream.sendall(f"SINK {size}\n".encode())
        for _ in range(size // len(BLOCK)):
            stream.sendall(BLOCK)
            digest.update(BLOCK)
        if exact(stream, digest.digest_size) != digest.digest():
            raise RuntimeError("TCP upload checksum mismatch")
    finished = time.monotonic()
    return {"bytes": size, "seconds": finished - started, "finished_monotonic": finished}


def metrics(packets, deadline_ms):
    values = sorted(p["rtt_ms"] for p in packets if p["rtt_ms"] is not None)

    def quantile(fraction):
        return values[math.ceil(fraction * len(values)) - 1] if values else None

    longest_late = late = 0
    for packet in packets:
        rtt = packet["rtt_ms"]
        late = late + 1 if rtt is None or rtt > deadline_ms else 0
        longest_late = max(longest_late, late)
    return {
        "sent": len(packets), "received": len(values),
        "within_deadline": sum(v <= deadline_ms for v in values),
        "p50_ms": quantile(.50), "p95_ms": quantile(.95), "p99_ms": quantile(.99),
        "max_ms": max(values, default=None), "longest_late_packets": longest_late,
    }


def write_result(path, result):
    path.write_text(json.dumps(result, indent=2) + "\n")


def worker(case_file):
    path = Path(case_file)
    case = json.loads(path.read_text())
    directory = path.parent
    target_host = "127.0.0.1"
    targets = case["targets"]
    with contextlib.ExitStack() as stack:
        traces = {}
        children = []
        sessions = {"bulk": 21880}
        if case["split"]:
            sessions["voice"] = 21881
        for role, port in sessions.items():
            config = case["probe_config"] | {"socks_port": port, "seconds": 115}
            probe_path = directory / f"{role}.config.json"
            write_result(probe_path, config)
            trace = traces[role] = directory / f"{role}.trace"
            out = stack.enter_context(trace.open("w"))
            children.append(stack.enter_context(process(
                [case["probe"], str(probe_path), "--interactive"],
                stdin=subprocess.PIPE, stdout=out, stderr=subprocess.STDOUT,
            )))
        for child, trace in zip(children, traces.values()):
            wait_ready(child, trace)
        start = time.monotonic() + .8
        duration = 11
        with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
            audio = pool.submit(voice, sessions.get("voice", sessions["bulk"]), target_host,
                                targets["udp_port"], duration, start, start_at=start)
            # Unpaced bulk TCP and paced large UDP are tested separately
            # to see whether bulk UDP should share the voice SSH.
            if case["load"] in ("download", "upload"):
                time.sleep(max(0, start - time.monotonic()))
                size = 4 * 1024 * 1024 if case["scenario"] != "narrow" else 2 * 1024 * 1024
                count = 4
                function = tcp_upload if case["load"] == "upload" else tcp_load
                common = (sessions["bulk"], targets["tcp_port"], size // count)
                arguments = common if function is tcp_upload else (common[0], target_host, *common[1:])
                bulk = [pool.submit(function, *arguments) for _ in range(count)]
                transfers = [job.result() for job in bulk]
                finish = max(t["finished_monotonic"] for t in transfers)
                bulk_stats = {"tcp_mbps": sum(t["bytes"] for t in transfers) * 8 / (finish - start) / 1e6}
            elif case["load"] != "idle":
                bulk_port = sessions.get("voice", sessions["bulk"]) if case["load"] == "udp-shared" else sessions["bulk"]
                bulk_result = voice(bulk_port, target_host, targets["udp_port"], duration, start,
                                    size=1200, packets_per_second=case["bulk_udp_pps"], start_at=start)
                finish = start + duration
                bulk_stats = {"bulk_udp": metrics(bulk_result["packets"], 700),
                              "bulk_udp_offer_mbps_per_direction": 1200 * case["bulk_udp_pps"] * 8 / 1e6}
            else:
                finish = start + duration
                bulk_stats = {}
            audio_result = audio.result()
        stop_ms = min(duration * 1000, (finish - start) * 1000)
        packets = [p for p in audio_result["packets"] if 0 <= p["sent_ms"] < stop_ms]
        for child in children:
            if child.poll() is not None:
                raise RuntimeError("SSH probe exited during measurement")
        samples = {
            role: [json.loads(row[7:]) for row in trace.read_text().splitlines() if row.startswith("SAMPLE ")]
            for role, trace in traces.items()
        }
        if not all(samples.values()):
            raise RuntimeError("Missing SSH queue samples")
        result = {
            **{key: case[key] for key in ("route", "scenario", "repeat", "split", "load")},
            **bulk_stats, "voice": metrics(packets, 200 if case["scenario"] != "narrow" else 350),
            "voice_first_3s": metrics([p for p in packets if p["sent_ms"] < 3000],
                                     200 if case["scenario"] != "narrow" else 350),
            "voice_within_700_ms": sum(p["rtt_ms"] is not None and p["rtt_ms"] <= 700 for p in packets),
            "packets": packets, "samples": samples,
        }
        write_result(directory / "result.json", result)


def setup_link(client_ns):
    command("ip", "link", "add", "st-server", "type", "veth", "peer", "name", "st-client", "netns", str(client_ns))
    enter = ["nsenter", "-t", str(client_ns), "-n", "--"]
    for prefix, interface, address in (([], "st-server", SERVER_IP), (enter, "st-client", CLIENT_IP)):
        command(*prefix, "ip", "link", "set", "lo", "up")
        command(*prefix, "ip", "addr", "add", address + "/30", "dev", interface)
        command(*prefix, "ip", "link", "set", "dev", interface, "up")
        command(*prefix, "ethtool", "-K", interface, "tso", "off", "gso", "off", "gro", "off")
    return enter


def shape_link(enter, scenario):
    delay, jitter, loss, rate, limit = SCENARIOS[scenario]
    for prefix, interface in (([], "st-server"), (enter, "st-client")):
        command(*prefix, "tc", "qdisc", "replace", "dev", interface, "root", "netem",
                "limit", str(limit), "delay", f"{delay}ms", f"{jitter}ms",
                "loss", f"{loss}%", "rate", f"{rate}mbit")


def qdiscs(enter):
    return {
        role: json.loads(command(*prefix, "tc", "-j", "-s", "qdisc", "show", "dev", interface))[0]
        for role, prefix, interface in (("server", [], "st-server"), ("client", enter, "st-client"))
    }


def measure_case(enter, case_path):
    before = qdiscs(enter)
    samples = []
    with process([*enter, sys.executable, str(Path(__file__).resolve()), "--case", str(case_path)]) as job:
        deadline = time.monotonic() + 110
        while job.poll() is None:
            if time.monotonic() >= deadline:
                raise TimeoutError(f"Workload timed out: {case_path.parent.name}")
            current = qdiscs(enter)
            samples.append({"at_ms": time.time_ns() // 1_000_000,
                            **{role: {key: q[key] for key in ("bytes", "backlog", "qlen", "drops")}
                               for role, q in current.items()}})
            time.sleep(.25)
        if job.returncode:
            raise subprocess.CalledProcessError(job.returncode, job.args)
    return {"link_before": before, "link_after": qdiscs(enter), "link_samples": samples}


def ssh_fixture(directory, agent):
    for name in ("client", "host"):
        command("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(directory / name))
    bin_dir = directory / ".local" / "bin"
    bin_dir.mkdir(parents=True)
    shutil.copy2(agent, bin_dir / "secret-tunnel-agent")
    config = directory / "sshd.conf"
    config.write_text(f"""
Port {SSH_PORT}
ListenAddress {SERVER_IP}
ListenAddress 127.0.0.1
HostKey {directory / 'host'}
AuthorizedKeysFile {directory / 'client.pub'}
PidFile {directory / 'sshd.pid'}
PermitRootLogin prohibit-password
PasswordAuthentication no
KbdInteractiveAuthentication no
AuthenticationMethods publickey
UsePAM no
AllowUsers root
AllowTcpForwarding local
PermitOpen 127.0.0.1:{SSH_PORT}
X11Forwarding no
ForceCommand env HOME={directory} /bin/sh -c "$SSH_ORIGINAL_COMMAND"
""".strip() + "\n")
    pin = command("ssh-keygen", "-lf", str(directory / "host.pub")).split()[1]
    return config, {"name": "Linux fixture", "port": SSH_PORT, "user": "root",
                    "key_path": str(directory / "client"), "host_key_sha256": pin}


def run(args):
    # Refuse to alter interfaces outside the outer unshare namespace.
    if os.readlink("/proc/self/ns/net") == os.readlink("/proc/1/ns/net"):
        raise RuntimeError("Run inside an isolated network namespace with unshare --net")
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    # OpenSSH checks the whole authorized_keys path, including its parents.
    with tempfile.TemporaryDirectory(prefix="st-ssh-load-", dir=Path.home()) as temp, contextlib.ExitStack() as stack:
        config, endpoint = ssh_fixture(Path(temp), args.agent)
        ns = stack.enter_context(process(["unshare", "--net", "--", "sleep", "7200"]))
        deadline = time.monotonic() + 4
        while os.readlink(f"/proc/{ns.pid}/ns/net") == os.readlink("/proc/self/ns/net"):
            if time.monotonic() > deadline:
                raise RuntimeError("Client namespace did not start")
            time.sleep(.02)
        enter = setup_link(ns.pid)
        sshd_log = stack.enter_context((args.output / "sshd.log").open("w"))
        sshd = shutil.which("sshd") or "/usr/sbin/sshd"
        stack.enter_context(process([sshd, "-D", "-e", "-f", str(config)], stderr=sshd_log))
        echo = stack.enter_context(process([sys.executable, str(Path(__file__).with_name("echo-server.py")),
                                            "--seconds", "7200"], stdout=subprocess.PIPE, text=True))
        targets = json.loads(echo.stdout.readline())
        for repeat in range(args.repeats):
            for scenario in args.scenarios:
                for route in args.routes:
                    for load in args.loads:
                        for split in (False, True) if repeat % 2 == 0 else (True, False):
                            if load == "udp-shared" and not split:
                                continue
                            shape_link(enter, scenario)
                            case_dir = args.output / f"{repeat}-{scenario}-{route}-{load}-{'two' if split else 'one'}"
                            case_dir.mkdir()
                            case_path = case_dir / "case.json"
                            probe_config = {"server": endpoint | {"host": SERVER_IP}, "jumps": []}
                            if route == "chain":
                                probe_config = {"server": endpoint | {"host": "127.0.0.1"},
                                                "jumps": [endpoint | {"host": SERVER_IP}]}
                            case = {"probe": str(args.probe), "probe_config": probe_config, "targets": targets,
                                    "repeat": repeat, "scenario": scenario, "route": route, "split": split,
                                    "load": load, "bulk_udp_pps": args.bulk_udp_pps}
                            write_result(case_path, case)
                            network = measure_case(enter, case_path)
                            result = json.loads((case_dir / "result.json").read_text())
                            result.update(network)
                            write_result(case_dir / "result.json", result)
                            print(json.dumps({key: value for key, value in result.items()
                                              if key not in ("packets", "samples", "link_before", "link_after", "link_samples")}),
                                  flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agent", type=lambda p: Path(p).resolve())
    parser.add_argument("--probe", type=lambda p: Path(p).resolve())
    parser.add_argument("--output", type=lambda p: Path(p).resolve())
    parser.add_argument("--case")
    parser.add_argument("--repeats", type=int, choices=range(1, 6), default=2)
    parser.add_argument("--scenarios", choices=SCENARIOS, nargs="+", default=["lossy"])
    parser.add_argument("--routes", choices=["direct", "chain"], nargs="+", default=["chain"])
    parser.add_argument("--loads", choices=["idle", "download", "upload", "udp", "udp-shared"], nargs="+", default=["download"])
    parser.add_argument("--bulk-udp-pps", type=int, choices=range(1, 1001), metavar="1..1000", default=150)
    args = parser.parse_args()
    if args.case:
        worker(args.case)
    elif not all((args.probe, args.agent, args.output)):
        parser.error("--probe, --agent and --output are required")
    else:
        run(args)


if __name__ == "__main__":
    main()
