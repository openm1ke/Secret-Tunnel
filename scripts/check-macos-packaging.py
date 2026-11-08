#!/usr/bin/env python3
"""Exercise the production ZIP/DMG packager before compiling the full app."""
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile

REPO = Path(__file__).resolve().parent.parent


def main():
    with tempfile.TemporaryDirectory(prefix="secret-tunnel-package-check-") as directory:
        root = Path(directory)
        script = root / "scripts/package-macos.sh"
        script.parent.mkdir()
        shutil.copyfile(REPO / "scripts/package-macos.sh", script)
        app = root / "src-tauri/target/universal-apple-darwin/release/bundle/macos/Secret Tunnel.app"
        binary = app / "Contents/MacOS/secret-tunnel"
        binary.parent.mkdir(parents=True)
        (app / "Contents/Info.plist").write_bytes(plistlib.dumps({
            "CFBundleExecutable": "secret-tunnel",
            "CFBundleIdentifier": "dev.secrettunnel.packagecheck",
            "CFBundleName": "Secret Tunnel", "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": "0.2.2", "CFBundleVersion": "0.2.2",
        }))
        source = root / "probe.c"
        source.write_text("int main(void) { return 0; }\n")
        subprocess.run(["xcrun", "clang", "-arch", "arm64", "-arch", "x86_64",
                        "-mmacosx-version-min=13.0", str(source), "-o", str(binary)], check=True)
        for name in ["server/secret-tunnel-agent-linux-x86_64", "README-macOS.md", "LICENSE.txt",
                     "licenses/aarch64-apple-darwin/rust-dependencies.json",
                     "licenses/x86_64-apple-darwin/rust-dependencies.json"]:
            resource = app / "Contents/Resources" / name
            resource.parent.mkdir(parents=True, exist_ok=True)
            resource.write_text("Packaging fixture\n")
        subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(app)], check=True)
        subprocess.run(["bash", str(script), "0.2.2"], check=True)
        # The architecture gate must reject a single-architecture application.
        thin = root / "arm64-only"
        subprocess.run(["xcrun", "lipo", str(binary), "-thin", "arm64", "-output", str(thin)], check=True)
        thin.replace(binary)
        subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(app)], check=True)
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
        rejected = subprocess.run(["bash", str(script), "0.2.2"], capture_output=True)
        if rejected.returncode == 0:
            raise RuntimeError("Packager accepted a non-universal application")
    print("PASS: universal packaging, signature, ZIP/DMG integrity, rejection of ARM64-only app")


if __name__ == "__main__":
    main()
