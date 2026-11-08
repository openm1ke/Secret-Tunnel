#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="${1:?Usage: package-macos.sh VERSION}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.-]+)?$ ]] || exit 2
app="$repo/src-tauri/target/universal-apple-darwin/release/bundle/macos/Secret Tunnel.app"
release="$repo/artifacts/release"
stage="$(mktemp -d "${TMPDIR:-/tmp}/secret-tunnel-dmg.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")" = "$version"
lipo -verify_arch arm64 x86_64 "$app/Contents/MacOS/secret-tunnel"
codesign --verify --deep --strict --verbose=2 "$app"
for file in server/secret-tunnel-agent-linux-x86_64 licenses/aarch64-apple-darwin/rust-dependencies.json licenses/x86_64-apple-darwin/rust-dependencies.json README-macOS.md LICENSE.txt; do
    test -f "$app/Contents/Resources/$file"
done
mkdir -p "$release"
ditto -c -k --sequesterRsrc --keepParent "$app" "$release/SecretTunnel-$version-macos-universal.zip"
unzip -tq "$release/SecretTunnel-$version-macos-universal.zip"
ditto "$app" "$stage/Secret Tunnel.app"
ln -s /Applications "$stage/Applications"
hdiutil create -volname 'Secret Tunnel' -srcfolder "$stage" -ov -format UDZO "$release/SecretTunnel-$version-macos-universal.dmg"
hdiutil verify "$release/SecretTunnel-$version-macos-universal.dmg"
echo 'PASS: universal arm64/x86_64, ad-hoc signature, bundled resources, ZIP and DMG'
