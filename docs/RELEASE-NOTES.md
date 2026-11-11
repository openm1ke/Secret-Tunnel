Secret Tunnel v0.3.13 keeps the tunnel fast and responsive while something downloads or a call runs. It includes the unpublished 0.3.8–0.3.12 builds.

- **Lower latency under load.** The tunnel no longer parks up to a second of data in front of everything else. On a phone, Speedtest's loaded ping fell from 77–188 ms to 55–101 ms with download speed unchanged (61–77 Mbit/s). On the test bench with sixteen transfers on an 80 Mbit/s link and a 50 ms path, a request on another connection took 71 instead of 174 ms while uploading; on a 10 Mbit/s link during a call 82 instead of 164 ms, with call latency slightly lower.
- **Small requests skip the queue.** Pings, API calls and the first bytes of a new connection no longer wait behind bulk downloads, during a call too.
- **No more speed cap from the Balanced and Voice profiles.** UDP priority used to limit TCP to 96 or 64 KiB per round trip (about 14 Mbit/s on a 50 ms path); it now follows queueing delay. QUIC downloads and DNS lookups no longer switch it on, so scrolling Instagram or loading pages does not throttle parallel downloads.
- **Faster on long routes.** The SSH channel window is 4 MiB on desktop and Android (iOS keeps 2 MiB): sixteen downloads at 200 ms RTT went from 43 to 83 Mbit/s on the bench.
- **DNS no longer stalls UDP.** The server agent resolves names in UDP destinations without holding back datagrams to known addresses, merges and caches lookups, and never sends a waiting datagram after the flow was closed.
- The Android log shows the path delay the tunnel follows.

Install the **server agent from this release** (0.3.10) on the final SSH server: the download direction is controlled by the agent. Profiles set to maximum throughput keep the previous TCP behaviour. [Audit and measurements](https://github.com/openm1ke/Secret-Tunnel/blob/v0.3.13/docs/CLAUDE-INDEPENDENT-AUDIT.md).

Packages include a signed Android APK, Windows setup and portable ZIP, macOS universal DMG and ZIP, and the Linux x86_64 agent archive. macOS per-app routing uses the separately installed ProxyBridge component.
