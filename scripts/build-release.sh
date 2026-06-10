#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="current"
EMBEDDED_SSH=0

for arg in "$@"; do
  case "$arg" in
    current|macos|linux|windows|all)
      TARGET="$arg"
      ;;
    --embedded|--embedded-ssh)
      EMBEDDED_SSH=1
      ;;
    -h|--help|help)
      TARGET="$arg"
      ;;
    *)
      echo "Unknown argument: $arg" >&2
      exit 2
      ;;
  esac
done

usage() {
  cat <<'EOF'
Usage:
  scripts/build-release.sh [current|macos|linux|windows|all] [--embedded]

Notes:
  Tauri desktop packaging is best done natively on each OS:
    macos   -> run on macOS
    linux   -> run on Linux
    windows -> run on Windows Git Bash / MSYS2 / PowerShell bash

  Cross-packaging Windows/Linux/macOS installers from one host is not handled by
  this script because Tauri bundles depend on native system toolchains.

  --embedded builds with Cargo feature embedded-ssh, enabling the experimental
  in-process russh engine in addition to the stable System OpenSSH engine.
EOF
}

host_os() {
  case "$(uname -s)" in
    Darwin) echo "macos" ;;
    Linux) echo "linux" ;;
    MINGW*|MSYS*|CYGWIN*) echo "windows" ;;
    *) echo "unknown" ;;
  esac
}

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

cleanup_macos_dmg_state() {
  [[ "$(host_os)" == "macos" ]] || return 0

  if command -v hdiutil >/dev/null 2>&1; then
    while IFS= read -r mount_point; do
      [[ -n "$mount_point" ]] || continue
      echo "==> Detaching stale Tauri DMG mount: $mount_point"
      hdiutil detach "$mount_point" >/dev/null 2>&1 || true
    done < <(hdiutil info | awk '/\/Volumes\/dmg\./ {print $NF}')
  fi

  find "$ROOT_DIR/src-tauri/target/release/bundle/macos" -maxdepth 1 -name 'rw.*.dmg' -delete 2>/dev/null || true
}

run_current_build() {
  local os
  os="$(host_os)"

  echo "==> Host OS: $os"
  need_cmd node
  need_cmd npm
  need_cmd cargo

  if [[ ! -d "$ROOT_DIR/node_modules" ]]; then
    echo "==> Installing npm dependencies"
    npm install
  fi

  cleanup_macos_dmg_state

  local tauri_args
  tauri_args=(build)
  if [[ "$EMBEDDED_SSH" == "1" ]]; then
    tauri_args+=(--features embedded-ssh)
    echo "==> Building Secret Tunnel with embedded Rust SSH"
  else
    echo "==> Building Secret Tunnel"
  fi
  if [[ "$os" == "macos" ]]; then
    tauri_args+=(--bundles app)
    npm run tauri -- "${tauri_args[@]}"
  else
    npm run tauri -- "${tauri_args[@]}"
  fi

  if [[ "$os" == "macos" ]]; then
    local app_path="$ROOT_DIR/src-tauri/target/release/bundle/macos/Secret Tunnel.app"
    if [[ -d "$app_path" ]] && command -v codesign >/dev/null 2>&1; then
      echo "==> Refreshing local macOS ad-hoc signature"
      codesign --remove-signature "$app_path" >/dev/null 2>&1 || true
      codesign --force --deep --sign - "$app_path"
      codesign --verify --deep --verbose=2 "$app_path"
    fi
  fi

  echo "==> Release artifacts"
  find "$ROOT_DIR/src-tauri/target/release/bundle" -maxdepth 3 \( \
    -name "Secret Tunnel.app" -o \
    -name "Secret Tunnel*.dmg" -o \
    -name "Secret Tunnel*.deb" -o \
    -name "Secret Tunnel*.rpm" -o \
    -name "Secret Tunnel*.AppImage" -o \
    -name "Secret Tunnel*.msi" -o \
    -name "Secret Tunnel*.exe" \
  \) -print 2>/dev/null || true
}

main() {
  cd "$ROOT_DIR"

  case "$TARGET" in
    -h|--help|help)
      usage
      ;;
    current)
      run_current_build
      ;;
    macos|linux|windows)
      if [[ "$(host_os)" != "$TARGET" ]]; then
        echo "Requested target '$TARGET', but this host is '$(host_os)'." >&2
        echo "Run this script on $TARGET, or use CI runners for each OS." >&2
        exit 2
      fi
      run_current_build
      ;;
    all)
      echo "'all' means: run this script once on macOS, once on Linux, and once on Windows." >&2
      echo "Native packaging for all three OSes from one machine is intentionally not attempted here." >&2
      exit 2
      ;;
    *)
      usage
      exit 2
      ;;
  esac
}

main "$@"
