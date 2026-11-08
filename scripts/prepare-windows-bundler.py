#!/usr/bin/env python3
"""Prepare the pinned Tauri NSIS tools through the official release asset API."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import time
import zipfile

REPO = Path(__file__).resolve().parent.parent
TOOLS = REPO / "src-tauri/target/.tauri"
# Match tauri-cli-v2.11.2/crates/tauri-bundler/src/bundle/windows/nsis/mod.rs.
# SHA-256 pins also match the official GitHub release asset digests.
ASSETS = [
    ("tauri-apps/binary-releases", "nsis-3.11", "nsis-3.11.zip",
     "c7d27f780ddb6cffb4730138cd1591e841f4b7edb155856901cdf5f214394fa1"),
    ("tauri-apps/nsis-tauri-utils", "nsis_tauri_utils-v0.5.3", "nsis_tauri_utils.dll",
     "5ba143b5db4a87d32d6e7802e033330aae56cbceabe0d1e3ba41948385ad4709"),
]


def matches(path, checksum):
    return path.is_file() and hashlib.sha256(path.read_bytes()).hexdigest() == checksum


def download(repository, tag, name, checksum):
    target = TOOLS / "downloads" / name
    target.parent.mkdir(parents=True, exist_ok=True)
    if matches(target, checksum):
        return target
    partial = target.with_suffix(target.suffix + ".part")
    for attempt in range(3):
        partial.unlink(missing_ok=True)
        try:
            result = subprocess.run([
                "gh", "release", "download", tag, "--repo", repository,
                "--pattern", name, "--output", str(partial), "--clobber",
            ], timeout=240, check=False)
            succeeded = result.returncode == 0
        except subprocess.TimeoutExpired:
            succeeded = False
        if succeeded:
            if not matches(partial, checksum):
                partial.unlink(missing_ok=True)
                raise RuntimeError(f"SHA-256 mismatch: {repository}/{name}")
            partial.replace(target)
            return target
        if attempt < 2:
            time.sleep(5)
    partial.unlink(missing_ok=True)
    raise RuntimeError(f"Official release asset download failed: {repository}/{name}")


def main():
    lock = json.loads((REPO / "package-lock.json").read_text())
    if lock["packages"]["node_modules/@tauri-apps/cli"]["version"] != "2.11.2":
        raise RuntimeError("Update NSIS pins when changing the Tauri CLI version")
    archive, plugin = [download(*asset) for asset in ASSETS]
    nsis = TOOLS / "NSIS"
    unpacked = TOOLS / "nsis-3.11"
    # Always extract the verified archive, including when an Actions cache is used.
    for directory in (nsis, unpacked):
        if directory.exists():
            shutil.rmtree(directory)
    with zipfile.ZipFile(archive) as bundle:
        bundle.extractall(TOOLS)
    unpacked.rename(nsis)
    destination = nsis / "Plugins/x86-unicode/additional/nsis_tauri_utils.dll"
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(plugin, destination)
    required = ["makensis.exe", "Bin/makensis.exe", "Include/MUI2.nsh",
                "Stubs/lzma_solid-x86-unicode", "Plugins/x86-unicode/additional/nsis_tauri_utils.dll"]
    for name in required:
        if not (nsis / name).is_file():
            raise RuntimeError(f"Missing NSIS tool: {name}")
    print("PASS: pinned NSIS 3.11 and Tauri plugin 0.5.3, official API, SHA-256 verified")


if __name__ == "__main__":
    main()
