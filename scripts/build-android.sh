#!/usr/bin/env bash
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
work=${ANDROID_BUILD_ROOT:-"$HOME/.cache/secret-tunnel-android"}
sdk=${ANDROID_HOME:?Set ANDROID_HOME to the Android SDK directory}
ndk=${ANDROID_NDK_HOME:-"$sdk/ndk/27.0.12077973"}
ndk=$(cd "$ndk" && pwd -P)
hev=${HEV_SOURCE_DIRECTORY:-"$work/hev-socks5-tunnel"}
revision=8e3b4cd7168fe64b64f8b58537b5623fb6a7074d
mkdir -p "$work"
if [[ ! -d "$hev/.git" ]]; then
    git clone --no-checkout https://github.com/heiher/hev-socks5-tunnel.git "$hev"
    git -C "$hev" checkout --detach "$revision"
    git -C "$hev" submodule update --init --recursive
fi
[[ $(git -C "$hev" rev-parse HEAD) == "$revision" ]] || { echo 'Unexpected hev revision' >&2; exit 1; }
[[ -z $(git -C "$hev" status --porcelain) ]] || { echo 'hev sources have local changes' >&2; exit 1; }
git -C "$hev" submodule status --recursive | awk '/^[+-U]/{bad=1} END {exit bad}'
case "$(uname -s)" in
    Darwin) ndk_host=darwin-x86_64 ;;
    Linux) ndk_host=linux-x86_64 ;;
    *) echo 'Run this script on macOS, Linux, or in WSL' >&2; exit 1 ;;
esac
llvm="$ndk/toolchains/llvm/prebuilt/$ndk_host/bin"
[[ -f "$ndk/source.properties" && -f "$llvm/clang-18" ]] || { echo 'Expected Android NDK 27 toolchain directory' >&2; exit 1; }
# Older SDKs copied to NTFS can retain read-only attributes which drvfs maps
# to unusable modes. Repair only this user's NDK, only when it is unusable.
if [[ "$ndk" == /mnt/* && -O "$ndk" ]] && { [[ ! -x "$llvm/aarch64-linux-android26-clang" ]] || [[ ! -x "$ndk/ndk-build" ]] || [[ ! -r "$ndk/source.properties" ]]; }; then
    echo 'Restoring read/write/execute access to the owned NDK on the Windows drive'
    chmod -R u+rwX "$ndk"
    chmod u+rwx "$ndk/ndk-build" "$ndk/prebuilt/linux-x86_64/bin/"* "$llvm/"*
fi
# Gradle also reads NDK metadata and launches objcopy when assembling release APKs.
if [[ "$ndk" == /mnt/* && -O "$ndk" ]]; then
    chmod u+rw "$ndk/source.properties" "$ndk/meta/abis.json" "$ndk/meta/platforms.json"
    chmod u+rx "$llvm/llvm-objcopy" "$llvm/llvm-readelf" "$llvm/llvm-ar" "$llvm/llvm-ranlib" "$ndk/build/ndk-build"
    if [[ ! -r "$ndk/build/core/build-local.mk" || ! -r "$ndk/build/tools/ndk_bin_common.sh" ]]; then
        chmod -R u+rwX "$ndk/build"
    fi
fi
"$llvm/clang" --version >/dev/null
export CARGO_TARGET_DIR="$work/rust-target"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$llvm/aarch64-linux-android26-clang"
export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$llvm/x86_64-linux-android26-clang"
export CC_aarch64_linux_android="$llvm/aarch64-linux-android26-clang"
export CC_x86_64_linux_android="$llvm/x86_64-linux-android26-clang"
export AR_aarch64_linux_android="$llvm/llvm-ar"
export AR_x86_64_linux_android="$llvm/llvm-ar"
export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-z,max-page-size=16384"
for pair in 'aarch64-linux-android arm64-v8a' 'x86_64-linux-android x86_64'; do
    read -r target abi <<< "$pair"
    rustup target add "$target"
    cargo build --manifest-path "$repo/android/native/Cargo.toml" --target "$target" --release --locked -j 2
    mkdir -p "$repo/android/app/src/main/jniLibs/$abi"
    cp "$CARGO_TARGET_DIR/$target/release/libsecret_tunnel_android.so" "$repo/android/app/src/main/jniLibs/$abi/"
done
hev_build="$work/hev-accounting-$revision"
hev_obj="$work/ndk-accounting-obj"
mkdir -p "$hev_build"
tar --exclude=.git -cf - -C "$hev" . | tar -xf - -C "$hev_build"
patch --fuzz=0 -d "$hev_build" -p1 < "$repo/android/ndk/hev-traffic.patch"
patch --fuzz=0 -d "$hev_build" -p1 < "$repo/android/ndk/hev-priority-udp.patch"
cp "$repo/android/ndk/"st-traffic.[ch] "$hev_build/src/"
"$ndk/ndk-build" NDK_PROJECT_PATH=null APP_BUILD_SCRIPT="$hev_build/Android.mk" REV_ID="$revision" \
    NDK_APPLICATION_MK="$repo/android/ndk/Application.mk" NDK_OUT="$hev_obj" \
    NDK_LIBS_OUT="$repo/android/app/src/main/jniLibs" -j 2 hev-socks5-tunnel
for abi in arm64-v8a x86_64; do
    cp "$hev_obj/local/$abi/libhev-socks5-tunnel.so" "$repo/android/app/src/main/jniLibs/$abi/"
done
# Preserve dependency notices in the installable application.
assets="$repo/android/app/src/main/assets/licenses"
mkdir -p "$assets"
cp "$repo/LICENSE" "$assets/Secret-Tunnel.txt"
cp "$hev/LICENSE" "$assets/hev-socks5-tunnel.txt"
cp "$hev/src/core/LICENSE" "$assets/hev-socks5-core.txt"
for module in hev-task-system lwip yaml; do
    notice=$(find "$hev/third-part/$module" -maxdepth 1 -type f \( -iname 'license*' -o -iname 'copying*' \) | head -1)
    [[ -n "$notice" ]] || { echo "Missing $module license" >&2; exit 1; }
    cp "$notice" "$assets/$module.txt"
done
printf '%s\n' "$revision" > "$repo/android/app/src/main/assets/hev-revision.txt"
python3 "$repo/scripts/collect-android-licenses.py" "$assets"
export GRADLE_USER_HOME=${GRADLE_USER_HOME:-"$work/gradle-home"}
export JAVA_HOME=${JAVA_HOME:?Set JAVA_HOME to JDK 17}
[[ -f "$JAVA_HOME/bin/java" && -f "$JAVA_HOME/release" ]] || { echo 'JAVA_HOME must point to a JDK installation' >&2; exit 1; }
if [[ "$JAVA_HOME" == /mnt/* && -O "$JAVA_HOME" ]] && { [[ ! -r "$JAVA_HOME/lib/jvm.cfg" ]] || [[ ! -x "$JAVA_HOME/bin/java" ]]; }; then
    echo 'Restoring access to the owned JDK on the Windows drive'
    chmod -R u+rwX "$JAVA_HOME"
    chmod u+rwx "$JAVA_HOME/bin/"*
fi
# drvfs may allow java itself while denying executable mappings of JDK .so files.
# Lint's icon checks load AWT even in headless builds.
if [[ "$JAVA_HOME" == /mnt/* && -O "$JAVA_HOME" ]]; then
    chmod u+rx "$JAVA_HOME/lib/"*.so "$JAVA_HOME/lib/server/"*.so
fi
export PATH="$JAVA_HOME/bin:$PATH"
if [[ -n ${GRADLE_CMD:-} ]]; then gradle=("$GRADLE_CMD")
elif [[ -x "$work/gradle-local/bin/gradle" ]]; then gradle=("$work/gradle-local/bin/gradle")
else gradle=(bash "$repo/android/gradlew"); fi
variant=${ANDROID_BUILD_TYPE:-debug}
case "$variant" in debug) tasks=(:app:testDebugUnitTest :app:assembleDebug :app:lintDebug);; release) tasks=(:app:testReleaseUnitTest :app:assembleRelease :app:lintRelease);; *) echo 'ANDROID_BUILD_TYPE must be debug or release' >&2; exit 1;; esac
if [[ ${ST_BUILD_TRAFFIC_PROBE:-0} == 1 ]]; then tasks+=(:traffic-probe:assembleDebug); fi
"${gradle[@]}" -p "$repo/android" --no-daemon "${tasks[@]}"
mkdir -p "$repo/artifacts/android"
apk="$repo/artifacts/android/SecretTunnel-$variant.apk"
cp "$repo/android/app/build/outputs/apk/$variant/app-$variant.apk" "$apk"
python3 "$repo/scripts/check-android-apk.py" "$apk"
"$JAVA_HOME/bin/java" -jar "$sdk/build-tools/34.0.0/lib/apksigner.jar" verify --verbose "$apk"
if command -v sha256sum >/dev/null 2>&1; then sha256sum "$apk"
else shasum -a 256 "$apk"; fi
