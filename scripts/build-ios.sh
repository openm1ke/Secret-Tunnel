#!/usr/bin/env bash
# Build the native iOS client (SwiftUI app + Packet Tunnel extension), iOS 15+.
#
#   scripts/build-ios.sh [sim|device|all]          (default: all)
#
# Environment:
#   ST_DEVELOPMENT_TEAM  Apple Team ID. Without it the device build is unsigned
#                        (compile/link check only); with it Xcode signs automatically.
#   ST_BUNDLE_PREFIX     Bundle ID prefix you own (default dev.secrettunnel).
#   HEV_SOURCE_DIRECTORY Existing hev-socks5-tunnel checkout (pinned revision).
set -euo pipefail

MODE="${1:-all}"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IOS_DIR="$ROOT_DIR/ios"
BUILD_DIR="$IOS_DIR/build"
WORK="${IOS_BUILD_ROOT:-$HOME/.cache/secret-tunnel-ios}"
HEV_REVISION=8e3b4cd7168fe64b64f8b58537b5623fb6a7074d
export IPHONEOS_DEPLOYMENT_TARGET=15.0
export ST_BUNDLE_PREFIX="${ST_BUNDLE_PREFIX:-dev.secrettunnel}"
export ST_DEVELOPMENT_TEAM="${ST_DEVELOPMENT_TEAM:-}"

for cmd in xcodebuild xcodegen cargo rustup git make libtool; do
  command -v "$cmd" >/dev/null || { echo "Missing required command: $cmd" >&2; exit 1; }
done
rustup target add aarch64-apple-ios aarch64-apple-ios-sim >/dev/null

mkdir -p "$WORK" "$BUILD_DIR/include" "$BUILD_DIR/lib/iphoneos" "$BUILD_DIR/lib/iphonesimulator"

# --- hev-socks5-tunnel (same pinned revision as Android) ---------------------
hev="${HEV_SOURCE_DIRECTORY:-$WORK/hev-socks5-tunnel}"
if [[ ! -d "$hev/.git" ]]; then
  git clone --no-checkout https://github.com/heiher/hev-socks5-tunnel.git "$hev"
  git -C "$hev" checkout --detach "$HEV_REVISION"
  git -C "$hev" submodule update --init --recursive
fi
[[ $(git -C "$hev" rev-parse HEAD) == "$HEV_REVISION" ]] || { echo 'Unexpected hev revision' >&2; exit 1; }

build_hev() { # sdk out_dir
  local sdk=$1 out=$2 src="$WORK/hev-build-$1"
  if [[ -f "$out/libhev-socks5-tunnel.a" && "$out/libhev-socks5-tunnel.a" -nt "$hev/.git/HEAD" ]]; then
    return
  fi
  rm -rf "$src" && mkdir -p "$src"
  tar --exclude=.git -cf - -C "$hev" . | tar -xf - -C "$src"
  local min="-m${sdk}-version-min=15.0"
  [[ $sdk == iphonesimulator ]] && min="-mios-simulator-version-min=15.0"
  make -C "$src" -j"$(sysctl -n hw.ncpu)" \
    PP="xcrun --sdk $sdk clang" CC="xcrun --sdk $sdk clang" \
    CFLAGS="-arch arm64 $min" LFLAGS="-arch arm64 $min" static >/dev/null
  libtool -static -o "$out/libhev-socks5-tunnel.a" \
    "$src/bin/libhev-socks5-tunnel.a" \
    "$src/third-part/lwip/bin/liblwip.a" \
    "$src/third-part/yaml/bin/libyaml.a" \
    "$src/third-part/hev-task-system/bin/libhev-task-system.a"
}
echo "==> hev-socks5-tunnel $HEV_REVISION"
build_hev iphoneos "$BUILD_DIR/lib/iphoneos"
build_hev iphonesimulator "$BUILD_DIR/lib/iphonesimulator"
cp "$hev/src/hev-main.h" "$BUILD_DIR/include/"
mkdir -p "$BUILD_DIR/licenses"
cp "$hev/LICENSE" "$BUILD_DIR/licenses/hev-socks5-tunnel.txt"

# --- Rust core ---------------------------------------------------------------
echo "==> Rust core (ios/native)"
for target in aarch64-apple-ios aarch64-apple-ios-sim; do
  cargo build --release --manifest-path "$IOS_DIR/native/Cargo.toml" --target "$target"
done
cp "$IOS_DIR/native/target/aarch64-apple-ios/release/libsecret_tunnel_ios.a" "$BUILD_DIR/lib/iphoneos/"
cp "$IOS_DIR/native/target/aarch64-apple-ios-sim/release/libsecret_tunnel_ios.a" "$BUILD_DIR/lib/iphonesimulator/"

# --- Xcode project -------------------------------------------------------------
echo "==> Xcode project"
(cd "$IOS_DIR" && xcodegen generate --spec project.yml --quiet)

DD="$BUILD_DIR/DerivedData"
xcb() { # run xcodebuild, keep the full log, show only diagnostics and the result
  local log="$BUILD_DIR/xcodebuild-$1.log"; shift
  if xcodebuild "$@" >"$log" 2>&1; then
    grep -E "warning: |BUILD SUCCEEDED" "$log" | grep -v "IDERunDestination\|IDESchemeActions" | sort -u || true
  else
    grep -E "error: |BUILD FAILED" "$log" | sort -u
    echo "Full log: $log" >&2
    return 1
  fi
}
if [[ $MODE == sim || $MODE == all ]]; then
  echo "==> Simulator build (unsigned; Network Extension does not run in Simulator)"
  xcb sim -project "$IOS_DIR/SecretTunnelIOS.xcodeproj" -scheme SecretTunnel \
    -configuration Debug -sdk iphonesimulator -destination 'generic/platform=iOS Simulator' \
    -derivedDataPath "$DD" CODE_SIGNING_ALLOWED=NO build
  echo "Simulator app: $DD/Build/Products/Debug-iphonesimulator/SecretTunnel.app"
fi
if [[ $MODE == device || $MODE == all ]]; then
  if [[ -n $ST_DEVELOPMENT_TEAM ]]; then
    echo "==> Device build signed for team $ST_DEVELOPMENT_TEAM"
    sign=(-allowProvisioningUpdates)
  else
    echo "==> Device build without signing (set ST_DEVELOPMENT_TEAM to sign and install)"
    sign=(CODE_SIGNING_ALLOWED=NO)
  fi
  xcb device -project "$IOS_DIR/SecretTunnelIOS.xcodeproj" -scheme SecretTunnel \
    -configuration Release -sdk iphoneos -destination 'generic/platform=iOS' \
    -derivedDataPath "$DD" "${sign[@]}" build
  echo "Device app: $DD/Build/Products/Release-iphoneos/SecretTunnel.app"
fi
