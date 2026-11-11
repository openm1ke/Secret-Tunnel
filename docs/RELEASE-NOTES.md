Secret Tunnel v0.3.7 fixes refused connections under load and raises the speed of single TCP connections over Embedded SSH.

- **Refused connections.** Idle UDP flows, one per DNS lookup behind the Android VPN, could exhaust the session budget at a quarter of the flow limit. New connections were then refused while the status still showed "Running". The budget now fits the advertised limit, DNS associations close 5 s after their reply, and the Android status shows a refusal counter.
- **Faster single connections.** A TCP connection was limited to 64 KiB per round trip: about 9.6 Mbit/s at 50 ms RTT on any link. Connections now grow up to 256 KiB while the path stays fast and keep 64 KiB when a queue builds up. On the test bench one connection at 50 ms RTT reached 39 Mbit/s, and p95 UDP latency in the shaped voice scenarios stayed within the previous range.
- **No slowdown from DNS.** The UDP priority budget engages only for a sustained run of short datagrams on one flow and ignores DNS. Six lookups every 2 s previously cut four parallel downloads from 156 to 14.6 Mbit/s; now they run at 150 Mbit/s.

Install the **server agent from this release** (0.3.4) on the final SSH server. The larger window needs both the new client and the new agent; mixed versions keep the previous behaviour. Its `--capabilities` response includes `"elastic_tcp": true`.

Measurements were taken on loopback with added delay and rate limits, without packet loss. Real calls and mobile networks are not yet verified with this build. [Audit and measurements](https://github.com/openm1ke/Secret-Tunnel/blob/v0.3.7/docs/CLAUDE-INDEPENDENT-AUDIT.md).

Packages include a signed Android APK, Windows setup and portable ZIP, macOS universal DMG and ZIP, and the Linux x86_64 agent archive. macOS per-app routing uses the separately installed ProxyBridge component.
