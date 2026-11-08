#!/usr/bin/env python3
"""Real per-app TCP/UDP check on macOS with the configured SSH server.

Requires approved ProxyBridge, an installed server UDP agent, the embedded
diagnostic examples and a release client. Stop GUI interception before running.
Does not edit the saved config, known_hosts, system proxy, or other routing apps.
"""
import argparse
import json
from pathlib import Path
import plistlib
import queue
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "artifacts/macos-integration"


class Worker:
    def __init__(self, args):
        self.process = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE, text=True, bufsize=1)
        self.events = queue.Queue()
        self.routes = []
        self.errors = []
        self.readers = [threading.Thread(target=self.stdout, daemon=True),
                        threading.Thread(target=self.stderr, daemon=True)]
        for reader in self.readers:
            reader.start()

    def stdout(self):
        for line in self.process.stdout:
            if any(word in line for word in ("TunnelProbe", "DirectProbe", "Rules cleared")):
                self.routes.append(line.strip())
            if line.startswith(("READY", "Redirecting traffic.")):
                self.events.put(line.strip())
        self.events.put("EOF")

    def stderr(self):
        for line in self.process.stderr:
            self.errors.append(line.strip())
            self.errors[:] = self.errors[-30:]

    def ready(self, prefix, timeout):
        value = self.events.get(timeout=timeout)
        if not value.startswith(prefix):
            raise RuntimeError("Worker exited before readiness: " + "\n".join(self.errors))

    def close(self, graceful=False):
        if self.process.poll() is None:
            if graceful:
                self.process.stdin.close()  # production guardian performs SIGTERM cleanup
            else:
                self.process.terminate()
            try:
                code = self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
                raise RuntimeError("Worker did not stop cleanly")
        else:
            code = self.process.returncode
        for reader in self.readers:
            reader.join(timeout=2)
        if graceful and code != 0:
            raise RuntimeError("Guardian cleanup failed: " + "\n".join(self.errors))


def run(args, timeout=20):
    result = subprocess.run([str(arg) for arg in args], capture_output=True, text=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip() or "Command failed")
    return result.stdout.strip()


def probe_app(name, identifier):
    app = OUT / (name + ".app")
    binary = app / "Contents/MacOS" / name
    binary.parent.mkdir(parents=True, exist_ok=True)
    (app / "Contents/Info.plist").write_bytes(plistlib.dumps({
        "CFBundleIdentifier": identifier, "CFBundleExecutable": name,
        "CFBundleName": name, "CFBundlePackageType": "APPL", "CFBundleVersion": "1",
    }))
    run(["clang", "-Wall", "-Wextra", "-O2", ROOT / "scripts/macos-network-probe.c", "-o", binary])
    run(["codesign", "--force", "--sign", "-", "--identifier", identifier, app])
    return binary


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", type=Path, default=Path.home() / ".config/secret-tunnel/config.json")
    parser.add_argument("--client", type=Path, default=ROOT / "src-tauri/target/release/secret-tunnel")
    args = parser.parse_args()
    config = json.loads(args.config.read_text())
    server = next(s for s in config["servers"] if s["id"] == config["selected_server_id"])
    server = {**server, "key_path": str(Path(server["key_path"]).expanduser())}
    secret_values = [server["host"], server["user"], server["key_path"]]
    embedded = guardian = None
    OUT.mkdir(parents=True, exist_ok=True)
    try:
        selected = probe_app("TunnelProbe", "dev.secrettunnel.udp-probe")
        direct = probe_app("DirectProbe", "dev.secrettunnel.direct-probe")
        examples = ROOT / "src-tauri/target/debug/examples"
        profile = run([examples / "check_macos_profile", "1082", "dev.secrettunnel.udp-probe"])
        with tempfile.NamedTemporaryFile(mode="w", suffix=".json", prefix="secret-tunnel-private-") as config_file:
            json.dump({"selected_server_id": server["id"], "servers": [server]}, config_file)
            config_file.flush()
            embedded = Worker([examples / "check_embedded", config_file.name])
            embedded.ready("READY", 20)
            print("PASS: embedded SSH authenticated with existing server trust", flush=True)
            guardian = Worker([args.client, "--macos-redirect-guardian",
                               "/Applications/ProxyBridge.app/Contents/MacOS/ProxyBridge"])
            guardian.process.stdin.write(profile + "\n")
            guardian.process.stdin.flush()
            guardian.ready("Redirecting traffic.", 60)
            print(run(["python3", ROOT / "scripts/check_socks_udp.py", "1082"], timeout=35), flush=True)
            print("Selected app: " + run([selected]), flush=True)
            print("Selected app: " + run([selected, "tcp"]), flush=True)
            print("Unselected app: " + run([direct]), flush=True)
            deadline = time.monotonic() + 5
            expected = [("UDP TunnelProbe", "SOCKS5"), ("TCP TunnelProbe", "SOCKS5"), ("UDP DirectProbe", "Direct")]
            while not all(any(a in line and b in line for line in guardian.routes) for a, b in expected):
                if time.monotonic() >= deadline:
                    raise RuntimeError("Missing per-app routing evidence: " + repr(guardian.routes))
                time.sleep(0.1)
            guardian.close(graceful=True)
            if not any("Rules cleared, tunnel stopped." in line for line in guardian.routes):
                raise RuntimeError("Extension did not confirm rule cleanup")
            embedded.close()
            print("After stop: " + run([selected]), flush=True)
            result = {"passed": True, "routes": guardian.routes,
                      "checks": ["SSH host trust", "TCP HTTPS SOCKS", "SOCKS UDP DNS", "association cleanup",
                                 "selected raw UDP", "selected raw TCP", "unselected direct UDP",
                                 "parent EOF extension cleanup", "direct UDP after stop"]}
            (OUT / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            for line in guardian.routes:
                print(line)
            print("PASS: real macOS per-app TCP/UDP over SSH and cleanup")
    except Exception as error:
        message = str(error)
        for value in secret_values:
            if value:
                message = message.replace(value, "[redacted]")
        raise SystemExit(message) from None
    finally:
        if guardian and guardian.process.poll() is None:
            guardian.close(graceful=True)
        if embedded and embedded.process.poll() is None:
            embedded.close()


if __name__ == "__main__":
    main()
