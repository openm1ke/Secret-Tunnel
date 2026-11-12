Secret Tunnel v0.3.18 makes the tunnel faster for single connections and QUIC, and stops mistaking downloads for calls.

- **One download or upload is no longer capped at 40 Mbit/s.** Each TCP connection may now keep 1 MiB in flight instead of 256 KiB: on the test bench a single transfer went from 40 to 154 Mbit/s on a 50 ms path and from 10 to 41 Mbit/s on a 200 ms route. The tunnel judges congestion by the delay its own data sees, so the queue stays short with the larger window.
- **QUIC and other UDP traffic no longer loses packets in the tunnel.** A UDP flow may now have 512 datagrams in flight instead of 192. A phone lost 6% of its incoming UDP datagrams (YouTube, Instagram, Google use QUIC); on the bench a 40–60 Mbit/s stream lost 15–62% and now loses almost nothing.
- **Scrolling and speed tests are no longer slowed down as if a call were running.** Call protection now needs voice in both directions; before, the QUIC traffic of a video feed switched it on and held TCP downloads at about 10 Mbit/s.
- iOS keeps its previous windows because of the system's memory limit for VPN extensions.

Install the **server agent from this release** (0.3.18; the agent and the apps carry the same version) on the final SSH server: it controls the download direction and announces the new UDP limit. [Audit and measurements](https://github.com/openm1ke/Secret-Tunnel/blob/v0.3.18/docs/CLAUDE-INDEPENDENT-AUDIT.md).

Packages include a signed Android APK, Windows setup and portable ZIP, macOS universal DMG and ZIP, and the Linux x86_64 agent archive. macOS per-app routing uses the separately installed ProxyBridge component.
