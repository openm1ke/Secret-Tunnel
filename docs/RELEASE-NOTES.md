Secret Tunnel v0.3.10 keeps downloads fast during calls and on long routes.

- **No more 14 Mbit/s cap with the Balanced profile.** While UDP priority is active, the TCP budget now follows queueing delay instead of a fixed 96 KiB per round trip. On the test bench, four downloads with a call running reached 157 instead of 15 Mbit/s on a free 50 ms path; voice latency on the shaped 5 and 2 Mbit/s routes stayed within 8%.
- **Faster downloads on long routes.** The SSH channel window is now 4 MiB on desktop and Android. Sixteen downloads at 200 ms RTT went from 42.6 to 83 Mbit/s.
- **DNS no longer stalls UDP.** The server agent resolves names in UDP destinations without holding back datagrams to known addresses, merges and caches lookups, and never sends a waiting datagram after the flow was closed.

Install the **server agent from this release** (0.3.7) on the final SSH server. [Audit and measurements](https://github.com/openm1ke/Secret-Tunnel/blob/v0.3.10/docs/CLAUDE-INDEPENDENT-AUDIT.md).

Packages include a signed Android APK, Windows setup and portable ZIP, macOS universal DMG and ZIP, and the Linux x86_64 agent archive. macOS per-app routing uses the separately installed ProxyBridge component.
