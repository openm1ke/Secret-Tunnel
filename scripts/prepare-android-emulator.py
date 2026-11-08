#!/usr/bin/env python3
"""Import a local SSH fixture into a debug emulator build without embedding keys.

Validates the host using the existing OpenSSH trust store, copies the private key
directly to app-private storage over adb stdin, then encrypts it using Keystore
in a test-only instrumentation method. Does not modify authorized_keys.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess

PACKAGE = "dev.secrettunnel.android"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--serial", default="emulator-5554")
    parser.add_argument("--config", type=Path, default=Path.home() / ".config/secret-tunnel/config.json")
    args = parser.parse_args()
    adb = ["adb", "-s", args.serial]
    qemu = subprocess.check_output(adb + ["shell", "getprop", "ro.kernel.qemu"], text=True).strip()
    if qemu != "1":
        raise SystemExit("This helper only provisions an Android emulator")
    config = json.loads(args.config.read_text())
    server = next(s for s in config["servers"] if s["id"] == config["selected_server_id"])
    key = Path(server["key_path"]).expanduser()
    secrets = [server["host"], server["user"], str(key)]
    try:
        ssh = ["ssh", "-v", "-o", "ControlMaster=no", "-o", "ControlPath=none", "-o", "BatchMode=yes",
               "-o", "StrictHostKeyChecking=yes", "-o", "IdentitiesOnly=yes", "-o", "ConnectTimeout=8",
               "-p", str(server["port"]), "-i", str(key), "--", server["user"] + "@" + server["host"],
               "curl -fsS --max-time 10 https://api.ipify.org"]
        verified = subprocess.run(ssh, capture_output=True, text=True, timeout=25)
        if verified.returncode:
            raise RuntimeError("Trusted SSH verification failed: " + verified.stderr[-900:])
        fingerprint = re.search(r"Server host key: \S+ (SHA256:[A-Za-z0-9+/]{43})", verified.stderr)
        if not fingerprint:
            raise RuntimeError("Could not read the strictly verified OpenSSH host fingerprint")
        fixture = {"host": server["host"], "port": server["port"], "user": server["user"],
                   "fingerprint": fingerprint.group(1)}
        def private_write(name, payload):
            command = f"run-as {PACKAGE} sh -c 'umask 077; mkdir -p files; cat > files/{name}'"
            subprocess.run(adb + ["shell", command], input=payload, capture_output=True, check=True)
        private_write("bootstrap-test.key", key.read_bytes())
        private_write("integration-fixture.json", json.dumps(fixture).encode())
        result = subprocess.run(adb + ["shell", "am", "instrument", "-w", "-r", "-e", "class",
            "dev.secrettunnel.android.VpnFixtureTest#importVerifiedEmulatorFixture",
            PACKAGE + ".test/androidx.test.runner.AndroidJUnitRunner"], capture_output=True, text=True, timeout=45)
        if result.returncode or "OK (1 test)" not in result.stdout:
            raise RuntimeError("Emulator fixture import failed: " + result.stdout[-1800:] + result.stderr[-500:])
        out = Path(__file__).resolve().parent.parent / "artifacts/android/qa"
        out.mkdir(parents=True, exist_ok=True)
        oracle = out / "server-egress.private.json"
        oracle.write_text(json.dumps({"egress_ip": verified.stdout.strip()}))
        oracle.chmod(0o600)
        print("PASS: trusted SSH, emulator-only private transfer, matching host pin, Keystore encryption; server unchanged")
    except Exception as error:
        message = str(error)
        for value in secrets:
            if value:
                message = message.replace(value, "[redacted]")
        raise SystemExit(message) from None
    finally:
        subprocess.run(adb + ["shell", "run-as", PACKAGE, "rm", "-f", "files/bootstrap-test.key", "files/integration-fixture.json"],
                       capture_output=True)


if __name__ == "__main__":
    main()
