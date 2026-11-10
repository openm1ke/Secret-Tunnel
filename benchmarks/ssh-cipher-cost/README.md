# SSH cipher CPU cost

Standalone release microbenchmark for the `ring 0.17.14` backend used by the Android client. It uses public synthetic keys and does not connect to a server or read application settings.

```sh
cargo run --release --locked --manifest-path benchmarks/ssh-cipher-cost/Cargo.toml > cipher-cost.jsonl
```

Each JSON line measures repeated encryption and authenticated decryption for a 160-byte, 1,200-byte or 16-KiB buffer. OpenSSH ChaCha20-Poly1305 includes its encrypted length field. AES-GCM uses a four-byte length as associated data. The program rotates cipher order across three 250-ms samples per size.

`seal_open_ns` includes one encryption and one decryption. `processed_mib_per_s` counts the payload twice. Network transfer, VPN processing, complete SSH framing and battery consumption are outside this measurement. Public keys and benchmark nonce sequences are fixtures, not an example for managing production keys.

To run on an Android device, cross-compile with the Android NDK and the `aarch64-linux-android` Rust target. Set `CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER`, `CC_aarch64_linux_android` and `AR_aarch64_linux_android` to the NDK toolchain, then build:

```sh
cargo build --release --locked --manifest-path benchmarks/ssh-cipher-cost/Cargo.toml --target aarch64-linux-android
adb push benchmarks/ssh-cipher-cost/target/aarch64-linux-android/release/st-cipher-micro /data/local/tmp/st-cipher-micro
adb shell chmod 700 /data/local/tmp/st-cipher-micro
adb shell /data/local/tmp/st-cipher-micro > android-cipher-cost.jsonl
adb shell rm -f /data/local/tmp/st-cipher-micro
```

If `CARGO_TARGET_DIR` is set, adjust the binary path. Run the benchmark independently of throughput measurements. Device scheduling, thermal state and hardware crypto support affect results. The Windows application uses the aws-lc backend by default, so this ring benchmark does not directly measure its encryption cost.

[Measured results and tunnel investigation](../../docs/SSH-CIPHERS-AND-LATENCY-2026-11-10.md).
