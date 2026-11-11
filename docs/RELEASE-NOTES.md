Secret Tunnel v0.3.14 makes a single connection up to four times faster.

- **One download or upload is no longer capped at 40 Mbit/s.** Each connection may now keep 1 MiB in flight instead of 256 KiB. On the test bench a single transfer went from 40 to 154 Mbit/s on a 50 ms path and from 10 to 41 Mbit/s on a 200 ms route; on an 80 Mbit/s link it now fills the link (74 instead of 39 Mbit/s). Large files, app updates and video that use one connection benefit most. Many parallel connections and calls behave as before.
- iOS keeps the previous per-connection window because of the system's memory limit for VPN extensions.

Install the **server agent from this release** (0.3.11) on the final SSH server for faster uploads. [Audit and measurements](https://github.com/openm1ke/Secret-Tunnel/blob/v0.3.14/docs/CLAUDE-INDEPENDENT-AUDIT.md).

Packages include a signed Android APK, Windows setup and portable ZIP, macOS universal DMG and ZIP, and the Linux x86_64 agent archive. macOS per-app routing uses the separately installed ProxyBridge component.
