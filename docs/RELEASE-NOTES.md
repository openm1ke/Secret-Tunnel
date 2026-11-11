Secret Tunnel v0.3.9 keeps downloads fast while UDP priority is active.

- With the Balanced profile, anything that looked like a call capped all TCP downloads at 96 KiB per round trip: about 14.5 Mbit/s on a phone, against 80-86 Mbit/s with Maximum speed. App reconnect bursts, speed-test latency probes and QUIC video triggered it, and small TCP messages queued behind the cap too.
- The budget now follows queueing delay. It starts at the profile size, grows while the path stays free and halves when a queue appears. On the test bench four downloads with a call running reached 157 instead of 15 Mbit/s on a free 50 ms path and 39 instead of 14 Mbit/s on a saturated 50 Mbit/s link. On the shaped 5 and 2 Mbit/s voice routes p95 stayed within 8% of the fixed budget.

Install the **server agent from this release** (0.3.6) on the final SSH server; the agent limits the download direction. [Audit and measurements](https://github.com/openm1ke/Secret-Tunnel/blob/v0.3.9/docs/CLAUDE-INDEPENDENT-AUDIT.md).

Packages include a signed Android APK, Windows setup and portable ZIP, macOS universal DMG and ZIP, and the Linux x86_64 agent archive. macOS per-app routing uses the separately installed ProxyBridge component.
