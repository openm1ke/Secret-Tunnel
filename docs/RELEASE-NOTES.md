Secret Tunnel v0.3.0 improves cancellation, reconnect and agent diagnostics for the shared STMUX003 transport.

- The embedded client verifies SSH authentication, agent capabilities and the STMUX003 handshake before enabling routes. Install the 0.3.0 agent before upgrading clients.
- The Linux x86_64 installer verifies SHA256 and exercises TCP and UDP before replacing the user-owned agent. It saves a checked backup for rollback.
- Desktop Stop cancels pending starts and reconnects. Configuration writes are atomic and retain a recovery copy. Android can restart a failed packet engine while keeping a healthy SSH session.
- The signed APK was installed over 0.2.3 on a realme RMX3630. It retained the server profile and nine selected apps. Selected and excluded test apps took their expected routes.
- See [measured TCP throughput, UDP RTT and resource use](PERFORMANCE-0.3.0.md).

Packages include a signed Android APK, Windows setup and portable ZIP, macOS universal DMG and ZIP, and the Linux x86_64 agent archive. macOS per-app routing uses the separately installed ProxyBridge component.
