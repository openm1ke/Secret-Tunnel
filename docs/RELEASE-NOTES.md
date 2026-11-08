SSH TCP/UDP tunneling for Windows and Android with per-application routing.

- **Android APK:** release build with a persistent private signing key; native VpnService, app checkboxes, SSH key import, host-key pinning, traffic chart and reconnect. Android 8+, ARM64/x86_64. Physical-device VPN testing is still pending; the common transport and APK packaging have been tested.
- **Windows setup:** application, interception engine, signed upstream WinDivert driver, licenses, server agent and offline Microsoft WebView2 installer.
- **Windows portable ZIP:** application, interception engine/driver, licenses and server agent. Requires an existing WebView2 Runtime. Run as administrator for interception.
- **Server archive:** static Linux x86_64 UDP-agent and user-only installation script. No public UDP listener, system service or changes to existing Xray/wstunnel are needed.

Windows supports regex domain routing with direct/tunnel actions and decision counters. Android currently routes by application; its domain-rule editor is not included yet.

Import your own SSH key and server settings; distributions contain no user credentials. Desktop EXE/setup currently have no commercial Authenticode signature. The WinDivert driver retains its verified upstream signature.

SHA256SUMS.txt covers the downloadable files. See the repository's docs/ANDROID.md and docs/WINDOWS-RELEASE.md for setup.
