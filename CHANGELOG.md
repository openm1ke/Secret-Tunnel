# Changelog

## 0.3.11 — 2026-11-11

- The session TCP budget follows delivery delay all the time, not only during a call. It starts at 1 MiB, doubles per round trip until `Delivered` returns late, then drops to what the path carried without a queue and grows by a quarter per round trip; never below 256 KiB. Before, 8 MiB could wait in the agent and the SSH path: 16 downloads on an 80 Mbit/s link with 50 ms RTT raised a ping on another connection to 107 ms (1 MiB SSH window) or 174 ms (4 MiB). Now 59 ms. Throughput on a bottleneck link is 3–5% lower; without a bottleneck it is unchanged.
- A flow with at most 32 KiB in flight draws on a separate 512 KiB reserve, so a ping reply, an API call or the start of a connection no longer waits for permits behind bulk downloads, also during a call.
- The uncongested path delay the budget compares against is the lowest of the last 10–20 s, so a path that became slower is not throttled for the rest of the session. Diagnostics show it as `tcp_flight.path_delay_ms`; Android shows it in the log.
- Profiles without a TCP budget (maximum throughput) are unchanged.

## 0.3.10 — 2026-11-11

- The server agent resolves names in UDP destinations outside the packet loop. A slow lookup no longer delays datagrams to known addresses in the same association, concurrent lookups of one name are merged and cached for 30 s, the waiting queue is bounded, and nothing waiting for a name is sent after the flow was closed.
- The default SSH channel window is 4 MiB on desktop and Android (iOS keeps 2 MiB). It bounds the download of the whole session: 16 flows at 200 ms RTT went from 42.6 to 83 Mbit/s on the test bench. Profiles with the reduced queue keep 1 MiB.
- Includes 0.3.9 (TCP budget follows queueing delay while UDP priority is active); its build was not published because a CI package mirror failed.

## 0.3.9 — 2026-11-11

- The Balanced and Voice profiles no longer cap TCP at a fixed 96 or 64 KiB per round trip while UDP priority is active. The budget starts there, grows by about one TCP chunk per round trip while deliveries return near the path delay and halves when a queue builds up. On a 50 ms path, four downloads with a call running reached 157 instead of 15 Mbit/s; voice p95 stayed within 8% of the fixed budget on the shaped 5 and 2 Mbit/s routes. Apps that merely look like a call (reconnect bursts, latency probes, QUIC) now cost little. Requires the agent from this release for downloads.

## 0.3.8 — 2026-11-11

- QUIC downloads no longer switch on the UDP priority budget. Their short ACKs looked like a call on a phone scrolling Instagram, which capped parallel TCP downloads. A flow now counts as a call only while it carries less than about 6 Mbit/s; voice and ordinary video calls still qualify. Requires the agent from this release on the server for downloads.

## 0.3.7 — 2026-11-11

- Fixed new connections being refused while the tunnel showed "Running". Idle UDP flows reserved larger windows than the session budget assumed, so Android ran out after 128 of its 512 flows. The budget now fits the advertised flow limit, DNS associations close 5 s after their reply, and refusals appear once per 10 s in the journal and as a counter in the Android status.
- TCP connections can grow from a 64 KiB to a 256 KiB window while the path stays fast. One connection at 50 ms RTT now carries about 39 instead of 9.6 Mbit/s. Slow links keep 64 KiB, so voice latency is unchanged. Both the client and the agent from this release are needed; mixed versions keep 64 KiB.
- The UDP priority budget now engages only for a sustained run of short datagrams on one flow, never for DNS. Page loads no longer cap concurrent TCP downloads for 15 s.
- Added a native iOS/iPadOS 15+ client with a Packet Tunnel extension (device-wide, IPv4-only, built from source).
- Independent audit with reproducible stands: [report](docs/CLAUDE-INDEPENDENT-AUDIT.md).

## 0.3.6 — 2026-11-10

- Added per-session UDP priority in Embedded SSH. Balanced uses a 96 KiB TCP flight budget during sustained short UDP traffic, Voice uses 64 KiB, and Maximum speed disables the extra budget. The first two profiles require the bundled agent on the destination server.
- Added full-route heartbeat, transport diagnostics and Android network handover recovery. The Android journal shows the negotiated budget and queue timings on both ends.
- Verified Windows SOCKS TCP and UDP, Android per-app VPN and concurrent clients with different settings. The benchmark records the remaining startup delay when bulk TCP and UDP begin together. [Validation](docs/QOS-SCHEDULING-2026-11-10.md).

## Earlier changes

- Added an Android APK target in the same repository: native per-app VpnService, shared Rust SSH/SOCKS TCP/UDP transport, key import protected by Android Keystore, host-key pinning, reconnect, traffic chart and logs. Added Linux/WSL build scripts and Android CI.
- Added protected transport sockets for Android and removed the coarse session lock after SSH authentication so independent channel opens do not block each other.

- Added installed-program discovery, searchable checkbox selection and app-family updater rules, plus manual exe selection and apply-without-restarting-SSH.
- Optimized UDP framing and server output batching, bounded DNS caching and idle-flow cleanup; echo integrity/throughput measurements are recorded in docs/APPLICATION-SELECTOR.md.

- Added Discord updater discovery and exact executable-path rules so Discord Update.exe can be proxied without capturing unrelated application updaters.

- Added experimental Windows application-routing UI and a ProxyBridge/WinDivert adapter; TCP/UDP interception validated with dedicated test processes, Discord voice validation pending.

- Added a local counted TCP relay for System OpenSSH traffic graphs on Windows, macOS and Linux, with bounded connections and cleanup on tunnel stop.

- Prevented SSH and curl console windows from appearing during Windows tunnel operations; debug clients now also use the Windows GUI subsystem.
- Added experimental SOCKS5 UDP ASSOCIATE over an existing embedded SSH connection and a standalone UDP relay agent.

- Added System OpenSSH and experimental Embedded Rust SSH engines.
- Added server, local proxy, and settings tabs.
- Added IPinfo Lite lookup, country flags, proxy IP masking, runtime logs, reconnect settings, and traffic chart.
- Added macOS/Linux/Windows build helper.
