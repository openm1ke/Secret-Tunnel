#!/usr/bin/env bash
# Build Secret Tunnel for the iOS Simulator (Apple Silicon).
# Note: the Simulator has no Network Extension support, so per-app interception
# cannot run there. This build covers the UI and the in-app Embedded SSH tunnel.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

chmod +x scripts/ios/swift
export PATH="$ROOT_DIR/scripts/ios:$PATH"

if [[ ! -d src-tauri/gen/apple ]]; then
  npm run tauri -- ios init
fi

npm run tauri -- ios build --target aarch64-sim --features embedded-ssh "$@"
