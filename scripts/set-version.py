#!/usr/bin/env python3
"""One version for the apps and the server agent.

    scripts/set-version.py 0.3.17      # write it everywhere
    scripts/set-version.py --check     # verify every file carries package.json's version
    scripts/set-version.py --check 0.3.17

Clients and the agent ship from the same commit and are released together, so
they carry the same number: an agent reporting 0.3.17 is the one built with the
0.3.17 apps. The Android versionCode follows scripts/android-version-code.py.
"""
import argparse
import importlib.util
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("vc", ROOT / "scripts" / "android-version-code.py")
vc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vc)

# (file, pattern with one group for the version, how many matches are expected)
FIELDS = [
    ("package.json", r'^  "version": "([^"]+)"', 1),
    ("package-lock.json", r'^  "version": "([^"]+)"', 1),
    ("package-lock.json", r'^    "": \{\n      "name": "[^"]+",\n      "version": "([^"]+)"', 1),
    ("src-tauri/tauri.conf.json", r'^  "version": "([^"]+)"', 1),
    ("src-tauri/Cargo.toml", r'^\[package\]\nname = "secret-tunnel"\nversion = "([^"]+)"', 1),
    ("src-tauri/Cargo.lock", r'^name = "secret-tunnel"\nversion = "([^"]+)"', 1),
    ("udp-agent/Cargo.toml", r'^\[package\]\nname = "secret-tunnel-agent"\nversion = "([^"]+)"', 1),
    ("udp-agent/Cargo.lock", r'^name = "secret-tunnel-agent"\nversion = "([^"]+)"', 1),
    ("android/app/build.gradle.kts", r'ST_VERSION_NAME"\) \?: "([^"]+)"', 1),
    ("ios/project.yml", r"^    MARKETING_VERSION: (\S+)$", 1),
]
CODE = ("android/app/build.gradle.kts", r'ST_VERSION_CODE"\)\?\.toIntOrNull\(\) \?: (\d+)')


def matches(text, pattern):
    return list(re.finditer(pattern, text, re.M))


def check(version):
    wrong = []
    for name, pattern, count in FIELDS:
        found = [m.group(1) for m in matches((ROOT / name).read_text(encoding="utf-8"), pattern)]
        if len(found) != count or any(value != version for value in found):
            wrong.append(f"{name}: {found or 'not found'}")
    name, pattern = CODE
    code = [m.group(1) for m in matches((ROOT / name).read_text(encoding="utf-8"), pattern)]
    if code != [str(vc.version_code(version))]:
        wrong.append(f"{name} versionCode: {code or 'not found'}, expected {vc.version_code(version)}")
    return wrong


def write(version):
    vc.version_code(version)  # validates the format
    for name, pattern, count in FIELDS + [(CODE[0], CODE[1], 1)]:
        path = ROOT / name
        text = path.read_text(encoding="utf-8")
        found = matches(text, pattern)
        if len(found) != count:
            sys.exit(f"{name}: expected {count} version fields, found {len(found)}")
        value = str(vc.version_code(version)) if pattern == CODE[1] else version
        for m in reversed(found):
            text = text[: m.start(1)] + value + text[m.end(1):]
        path.write_text(text, encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("version", nargs="?")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        version = args.version or json.loads((ROOT / "package.json").read_text())["version"]
        wrong = check(version)
        if wrong:
            sys.exit(f"Version {version} is not everywhere:\n  " + "\n  ".join(wrong))
        print(f"All apps and the agent are {version}")
        return
    if not args.version:
        parser.error("give a version, or --check")
    write(args.version)
    wrong = check(args.version)
    if wrong:
        sys.exit("\n".join(wrong))
    print(f"Set {args.version} everywhere (Android versionCode {vc.version_code(args.version)})")


if __name__ == "__main__":
    main()
