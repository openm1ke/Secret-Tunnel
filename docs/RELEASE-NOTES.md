Secret Tunnel v0.3.20 ships the desktop updates from 0.3.19 and fixes the Windows release check.

- **Desktop dashboard, statistics, and speed tests.** Follow the active route, review saved sessions and application traffic, and compare a tunnel speed test with a test over the computer's own connection.
- **Windows release build.** Flow attribution uses a single deadline, and its test waits for the result on Windows.

- **One download or upload is no longer capped at 40 Mbit/s.** Each TCP connection may now keep 1 MiB in flight instead of 256 KiB: on the test bench a single transfer went from 40 to 154 Mbit/s on a 50 ms path and from 10 to 41 Mbit/s on a 200 ms route. The tunnel judges congestion by the delay its own data sees, so the queue stays short with the larger window.
- **QUIC and other UDP traffic no longer loses packets in the tunnel.** A UDP flow may now have 512 datagrams in flight instead of 192. A phone lost 6% of its incoming UDP datagrams (YouTube, Instagram, Google use QUIC); on the bench a 40–60 Mbit/s stream lost 15–62% and now loses almost nothing.
- **Scrolling and speed tests are no longer slowed down as if a call were running.** Call protection now needs voice in both directions; before, the QUIC traffic of a video feed switched it on and held TCP downloads at about 10 Mbit/s.
- iOS keeps its previous windows because of the system's memory limit for VPN extensions.

The **0.3.20 server agent** runs on the final SSH server. It controls the download direction and announces its UDP window limit. [Audit and measurements](https://github.com/openm1ke/Secret-Tunnel/blob/v0.3.20/docs/CLAUDE-INDEPENDENT-AUDIT.md).

Packages include a signed Android APK, Windows setup and portable ZIP, macOS universal DMG and ZIP, and the Linux x86_64 agent archive. macOS per-app routing uses the separately installed ProxyBridge component.
