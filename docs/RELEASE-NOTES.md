Secret Tunnel v0.3.13 keeps the tunnel responsive while something downloads.

- **Lower latency under load, same speed.** The tunnel no longer parks up to a second of download data in front of everything else. On the test bench with sixteen transfers on an 80 Mbit/s link and a 50 ms path, a request on another connection took 82 instead of 107 ms while downloading and 72 instead of 174 ms while uploading; on a 200 ms route 268 instead of 369 ms; on a 10 Mbit/s link during a call 81 instead of 164 ms. Throughput stayed the same or rose slightly, and call latency improved a little.
- **Small requests skip the queue.** Pings, API calls and the first bytes of a new connection no longer wait behind bulk downloads, during a call too.

Install the **server agent from this release** (0.3.10) on the final SSH server: the download direction is controlled by the agent. Profiles set to maximum throughput keep the previous behaviour. [Audit and measurements](https://github.com/openm1ke/Secret-Tunnel/blob/v0.3.13/docs/CLAUDE-INDEPENDENT-AUDIT.md).

Packages include a signed Android APK, Windows setup and portable ZIP, macOS universal DMG and ZIP, and the Linux x86_64 agent archive. macOS per-app routing uses the separately installed ProxyBridge component.
