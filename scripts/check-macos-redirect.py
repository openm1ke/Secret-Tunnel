#!/usr/bin/env python3
"""Exercise the real macOS guardian with a fake CLI, without modifying networking."""
import json
import os
from pathlib import Path
import selectors
import subprocess
import sys
import tempfile
import textwrap


def main():
    binary = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix="secret-tunnel-guardian-") as directory:
        root = Path(directory)
        cli = root / "ProxyBridge"
        cli.write_text(f"#!{sys.executable}\n" + textwrap.dedent('''
            import json, pathlib, signal, sys, time
            root = pathlib.Path(__file__).parent
            assert sys.argv[1:] == ['--cli', '--profile', '-', '--verbose', '3']
            profile = json.load(sys.stdin)  # must receive EOF after the profile
            (root / 'received.json').write_text(json.dumps(profile))
            def stop(sig, frame):
                (root / 'cleaned').write_text('SIGTERM')
                if not profile.get('omit_ack'):
                    print('Rules cleared, tunnel stopped.', flush=True)
                sys.exit(0)
            signal.signal(signal.SIGTERM, stop)
            print('Redirecting traffic. Press Ctrl+C to stop.', flush=True)
            while True: time.sleep(0.05)
        '''))
        cli.chmod(0o700)
        profile = {"name": "Secret Tunnel", "proxyConfigs": [], "proxyRules": []}
        process = subprocess.Popen([str(binary), "--macos-redirect-guardian", str(cli)],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            process.stdin.write((json.dumps(profile) + "\n").encode())
            process.stdin.flush()
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                if not selector.select(timeout=8):
                    raise AssertionError("guardian did not launch CLI")
            assert process.stdout.readline().decode().startswith("Redirecting traffic.")
            assert process.poll() is None, "guardian exited while its parent pipe was open"
            assert json.loads((root / "received.json").read_text()) == profile
            # EOF is also what happens when the GUI crashes or is SIGKILLed.
            process.stdin.close()
            assert process.wait(timeout=10) == 0, process.stderr.read().decode()
            assert (root / "cleaned").read_text() == "SIGTERM"
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
        failed = subprocess.run([str(binary), "--macos-redirect-guardian", str(cli)],
                                input=b"invalid\n", capture_output=True, timeout=8)
        assert failed.returncode != 0 and b"ERROR:" in failed.stderr
        unconfirmed = subprocess.Popen([str(binary), "--macos-redirect-guardian", str(cli)],
                                      stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        unconfirmed.stdin.write(b'{"omit_ack":true}\n')
        unconfirmed.stdin.flush()
        assert unconfirmed.stdout.readline().startswith(b'Redirecting traffic.')
        unconfirmed.stdin.close()
        assert unconfirmed.wait(timeout=10) != 0, 'exit 0 without cleanup ACK must fail'
        assert b'ERROR:' in unconfirmed.stderr.read()
    print("PASS: CLI arguments, stdin profile + EOF, SIGTERM cleanup + ACK, rejection of missing ACK and invalid profile")


if __name__ == "__main__":
    if sys.platform != "darwin":
        sys.exit("This check requires macOS")
    main()
