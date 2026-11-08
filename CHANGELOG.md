# Changelog

## Unreleased

- Added an Android APK target in the same repository: native per-app VpnService, shared Rust SSH/SOCKS TCP/UDP transport, key import protected by Android Keystore, host-key pinning, reconnect, traffic chart and logs. Added Linux/WSL build scripts and Android CI.
- Added protected transport sockets for Android and removed the coarse session lock after SSH authentication so independent channel opens do not block each other.

- Added installed-program discovery, searchable checkbox selection and app-family updater rules, plus manual exe selection and apply-without-restarting-SSH.
- Optimized UDP framing and server output batching, bounded DNS caching and idle-flow cleanup; echo integrity/throughput measurements are recorded in docs/APPLICATION-SELECTOR.md.

- Added Discord updater discovery and exact executable-path rules so Discord Update.exe can be proxied without capturing unrelated application updaters.

- Added experimental Windows application-routing UI and a ProxyBridge/WinDivert adapter; TCP/UDP interception validated with dedicated test processes, Discord voice validation pending.

- Added a local counted TCP relay for System OpenSSH traffic graphs on Windows, macOS and Linux, with bounded connections and cleanup on tunnel stop.

- Prevented SSH and curl console windows from appearing during Windows tunnel operations; debug clients now also use the Windows GUI subsystem.
- Added experimental SOCKS5 UDP ASSOCIATE over an existing embedded SSH connection and a standalone UDP relay agent.

- Added System OpenSSH and experimental Embedded Rust SSH engines.
- Added server, local proxy, and settings tabs.
- Added IPinfo Lite lookup, country flags, proxy IP masking, runtime logs, reconnect settings, and traffic chart.
- Added macOS/Linux/Windows build helper.
