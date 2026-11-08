#!/usr/bin/env python3
"""Monotonic Android release codes, preserving the 0.2.0 -> 20000 base."""
import argparse
import re


def version_code(version: str, build: int = 0) -> int:
    match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?:-[A-Za-z0-9.-]+)?", version)
    if not match or build < 0:
        raise ValueError("Expected a semantic version and nonnegative build number")
    major, minor, patch = map(int, match.groups())
    if minor >= 100 or patch >= 100:
        raise ValueError("Minor and patch must be below 100")
    code = major * 1_000_000 + minor * 10_000 + patch * 100 + min(build, 99)
    if not 1 <= code <= 2_100_000_000:
        raise ValueError("Version exceeds Android's versionCode range")
    return code


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version")
    parser.add_argument("--build", type=int, default=0)
    args = parser.parse_args()
    print(version_code(args.version, args.build))
