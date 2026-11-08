Secret Tunnel v0.2.2 adds a native Android interface for managing SSH servers, local SOCKS5 profiles and selected applications.

- **Android:** separate Home, Servers, Proxies, Applications and Diagnostics screens; dark interface, native application icons and switches, current upload/download speeds, cumulative traffic and a rolling chart recorded by the VPN service.
- **Profiles:** multiple named SSH servers and local SOCKS5 profiles with configurable ports. Credentials and trusted fingerprints are isolated per server. Existing settings migrate automatically. A running VPN session keeps its original configuration until stopped.
- **SSH onboarding:** discover and confirm the server fingerprint without typing it; optionally register a dedicated device key using an existing authorized key, then verify the new login before saving it.
- **Validation:** Android compilation, lint and 16 KiB native alignment/signature checks; two traffic unit tests and six emulator integration/UI tests passed locally. The shared TCP/UDP transport was also tested against a real SSH server. Physical-device validation of this release is still pending.
- **Android updates:** versionCode now derives from the release version, retaining compatibility with v0.2.0 and allowing future releases to upgrade earlier builds.
- **Windows setup/portable:** application, interception engine, upstream signed WinDivert driver, licenses and Linux UDP agent. Setup includes the offline WebView2 installer; portable requires an existing WebView2 Runtime. Interception requires administrator rights.
- **Linux agent:** static x86_64 executable and user-only installation script. No public UDP listener or changes to existing Xray/wstunnel services are required.

Android 8+, ARM64/x86_64. Android routes by application; the regex domain-rule editor remains a Windows feature. Import your own SSH credentials; releases contain no user keys or server configuration. Desktop binaries currently have no commercial Authenticode signature. SHA256SUMS.txt covers the downloadable assets.
