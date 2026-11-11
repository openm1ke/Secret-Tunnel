Secret Tunnel v0.3.11 keeps the tunnel responsive while something downloads.

- **Low latency under load.** The tunnel no longer parks up to a second of download data in front of everything else. On the test bench, with sixteen downloads on an 80 Mbit/s link and 50 ms path, a request on another connection took 59 ms instead of 107–174 ms; on a 200 ms route 212 instead of 369 ms; on a 10 Mbit/s link with a call 91 instead of 164 ms. Call latency is unchanged. Throughput on a saturated link is 3–5% lower, and unchanged when the link is not the limit.
- **Small requests skip the queue.** Pings, API calls and the first bytes of a new connection no longer wait behind bulk downloads, during a call too.

Install the **server agent from this release** (0.3.8) on the final SSH server: the download direction is controlled by the agent. Profiles set to maximum throughput keep the previous behaviour. [Audit and measurements](https://github.com/openm1ke/Secret-Tunnel/blob/v0.3.11/docs/CLAUDE-INDEPENDENT-AUDIT.md).

Packages include a signed Android APK, Windows setup and portable ZIP, macOS universal DMG and ZIP, and the Linux x86_64 agent archive. macOS per-app routing uses the separately installed ProxyBridge component.
