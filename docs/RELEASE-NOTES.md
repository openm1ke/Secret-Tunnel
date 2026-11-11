Secret Tunnel v0.3.8 stops QUIC downloads from slowing other connections.

- On a phone scrolling Instagram, the short acknowledgements of QUIC video looked like a voice call and switched on the UDP priority budget. Parallel TCP downloads were then capped at about 15 Mbit/s for as long as the video played. A flow now counts as a call only while it carries less than about 6 Mbit/s. Voice and ordinary video calls still get priority; the shaped voice scenarios kept their previous latency.
- Includes everything from v0.3.7: no refused connections under load, TCP windows that grow up to 256 KiB on fast paths, and no slowdown from DNS lookups.

Install the **server agent from this release** (0.3.5) on the final SSH server; the agent limits the download direction. [Audit and measurements](https://github.com/openm1ke/Secret-Tunnel/blob/v0.3.8/docs/CLAUDE-INDEPENDENT-AUDIT.md).

Packages include a signed Android APK, Windows setup and portable ZIP, macOS universal DMG and ZIP, and the Linux x86_64 agent archive. macOS per-app routing uses the separately installed ProxyBridge component.
