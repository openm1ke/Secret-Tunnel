# Changelog

## 0.3.26 — 2026-11-14

- Android: the VPN restores SSH after switching between Wi-Fi and mobile data. The per-app «Блокировать при обрыве туннеля» checkbox keeps selected traffic from going direct while Android holds the VPN interface.
- Android: a network change interrupts the speed test, closes its connections and leaves the app running. An interrupted run is excluded from history.
- macOS, Windows and Android 10+: the «Голос» profile can use a separate SSH connection for UDP from marked apps. New UDP flows fall back to the primary SSH connection if the second one drops.
- The connection screen reports SSH and interception separately and checks TCP, UDP, DNS and IPv6 through SOCKS. Windows can match verified Dorion WebView2 child processes to their host app when using ProxyBridge.

## 0.3.25 — 2026-11-14

- Android: «Ещё» → «О программе» shows the installed version and checks GitHub for stable releases manually or at startup. New releases show their notes and download progress. The app verifies the APK checksum, version and signature before opening Android's installer.
- Android: check the agent version and install the matching Linux x86_64 agent through the configured SSH route.
- Android: setup instructions and a searchable knowledge base with an offline copy.

## 0.3.24 — 2026-11-13

- Desktop: «О программе» checks for updates and downloads the selected release with SHA-256 verification. On macOS it keeps the previous bundle and restores it if macOS rejects the launch request. On Windows the installed edition waits for the installer to finish successfully before reopening. The portable edition links to the release page.
- Desktop: check, install, or update the agent on a configured SSH server. The client downloads the agent that matches its own version, verifies it, and sends it over SSH. The server installer runs its self-test and preserves a copy of the previous binary for rollback.
- Desktop: searchable setup instructions and knowledge base, with an offline copy bundled in the app.
- Desktop: the application picker offers tiles, a table, and cards, with traffic and selection dates in the detailed views. The server screen shows the latest check result.

## 0.3.23 — 2026-11-12

- Android: «Скорость» with the desktop's tests — quick, full, bandwidth, browsing, mixed, latency and UDP, M-Lab NDT7 — through the tunnel or straight from the phone, on Cloudflare or the nearest LibreSpeed server, with history, exit country and a tunnel-vs-direct comparison. History uses the desktop's `speedtests.json` fields.
- Android: «Статистика» with tunnel sessions and each app's traffic for today, 7 or 30 days or all time, kept in `sessions.json` like on the desktop. Traffic that went through the proxy itself (speed tests, apps set to use it) shows as «Без перехвата».
- Android: the home screen shows the exit IP and country after connecting («Проверять прокси после подключения»), hidden until tapped («Скрывать IP-адреса»).
- Android: bottom tabs are now «Главная», «Приложения», «Скорость», «Статистика», «Ещё»; servers, proxies, settings and the journal are under «Ещё».
- Android: cipher, SSH queue, call priority and UDP windows moved from each proxy profile to «Ещё» → «Настройки» and apply to every connection, as on the desktop. The selected profile's values carry over on update. Server form labels match the desktop: «Порт SSH», «Пользователь», «Подключаться через».
- Desktop: «UDP · загрузка» and «UDP · отправка» in «Для опытных: встроенный SSH», the UDP windows Android already had. «По умолчанию» keeps 1 MiB / 512 slots for downloads and the agent's own window for uploads.
- Server and proxy forms: a Russian name fills the ID by transliteration ("Мой сервер" → `moy-server`), and the ID field can be edited. Before, a Russian name left the ID empty and «Сохранить» disabled, and typing in the ID field had no effect.
- Website: step-by-step install guide with pictures for macOS, Windows and Linux, from a hoster's root password to the first connection: https://openm1ke.github.io/Secret-Tunnel/guide.html

## 0.3.22 — 2026-11-12

- Windows: the application picker shows each program's icon, the one Explorer shows for the program or its shortcut. Entries added by process name only keep a letter badge.

## 0.3.21 — 2026-11-12

- New icon on desktop and Android: a tunnel mouth with the exit light. Android uses an adaptive icon with a monochrome layer for themed icons (Android 13+).
- Desktop tray and menu-bar icon. Closing the window keeps the app and the tunnel running there; the menu shows the tunnel's state and can open the window, connect, disconnect, check speed, open settings and quit. The icon follows the tunnel: a dark light when off, a yellow light when connected, and arches lighting up in turn while connecting. On macOS the Dock icon brings the window back. Setting: "Сворачивать в трей / строку меню при закрытии окна", on by default.
- The sidebar counts selected applications as soon as interception runs; it counted their rules until the Applications tab was opened.
- Statistics count programs an application starts under its own signature as that application: ChatGPT's `codex`, `Codex (Renderer)` and `node` show as ChatGPT.
- Website with a knowledge base, screenshots and every release: https://openm1ke.github.io/Secret-Tunnel/

## 0.3.20 — 2026-11-12

- Fixed the Windows release check for application traffic attribution. The client uses a single deadline for matching a flow, and the test waits for the attribution to finish.
- Includes the desktop interface, statistics, and speed tests from 0.3.19, and the call detection changes from 0.3.18.

## 0.3.19 — 2026-11-12

Desktop (macOS first; Windows shares the interface and the speed tests).

- Redesigned window: route ribbon from this computer through each server to the exit country, full-width traffic chart, a log dock, application icons, country flags on every platform.
- SSH to the server bypasses other VPNs' TUN interfaces on macOS (bound to the physical interface), so a running Clash no longer wraps the tunnel in a second VPN. Setting: "SSH мимо других VPN", on by default.
- Interception on macOS matches what the system reports. Rules carry the code-signing identifier of every program inside a selected app, not only bundle IDs: ChatGPT's agent `codex` is signed `codex` and went direct. DNS that intercepted apps send through the tunnel goes to 1.1.1.1: the system resolver (Clash's 114.114.114.114, a home router) does not answer from the server, which kept Discord at its update check.
- Programs a selected app starts (node, python, MCP servers, updaters) are followed while running, via kernel process events, and get a rule within about a second. Rules change without dropping connections: the old ProxyBridge CLI is stopped without cleanup and the new one only replaces the rules. Setting: "Подхватывать программы, запущенные выбранными приложениями".
- The application picker keeps the selection: shared helper IDs (Sparkle, Mozilla) no longer select or deselect other applications.
- Statistics: sessions with duration, route, exit country and traffic per application, kept locally.
- Journal: connections in plain words (through the tunnel, direct, blocked) with host names learned from tunnelled DNS, filters for events, connections, errors and service entries, IP hiding, repeats collapsed.
- Speed: Cloudflare, LibreSpeed and M-Lab NDT7 tests through the tunnel or over the computer's own connection, with gauges, latency under load, UDP round trips, the exit address and country, a comparison of the two, and a history that shows chains of servers.
- Quitting from the menu stops interception and the tunnel, as closing the window did.
- 0.3.18 (call detection) was not published; its change is part of this release.

## 0.3.18 — 2026-11-12

- A UDP flow counts as a call only while it carries short datagrams both ways, the quieter direction at least three per 500 ms and 30% of the busier one. Short datagrams one way are the ACKs of a download: on a phone the Instagram feed's QUIC flows held the call budget on 18 of 19 snapshots and capped TCP at 96 KiB per round trip, 7–12 Mbit/s on an 85 ms path, and Speedtest triggered it during half of its download. Bench, 4 TCP downloads plus a 2 Mbit/s QUIC-like flow with an ACK per two datagrams: call mode 8 s of 10 before, 0 now; a voice stream is still detected.
- During a call the TCP budget also decides once per round trip on the mean delay of its own bytes. Judging every sample halved it on each delay spike of a jittery path (Wi-Fi with 20 ms jitter) and held it at 96 KiB. On the jitter-free bench voice p95 is 95–99 ms instead of 89 ms on a 10 Mbit/s link with four transfers.

## 0.3.17 — 2026-11-11

- The apps and the server agent share one version number: agent 0.3.17 is the one built with the 0.3.17 apps (the agent was 0.3.13 next to app 0.3.16). `scripts/set-version.py X.Y.Z` writes it to every version file, including the Android versionCode, and the release workflow refuses a tag that any of them does not match (`--check`).
- The standard SSH queue is labelled 4 MiB in the Android and desktop settings; its window changed in 0.3.10.
- `scripts/deploy-agent.sh user@host [ssh options]` builds the Linux agent locally and installs it without the release workflow.

## 0.3.16 — 2026-11-11

- A UDP flow may have up to 512 datagrams unacknowledged instead of 192, and Android and desktop clients ask for a 1 MiB / 512-datagram downlink window by default (was 256 KiB / 192 and 128 KiB / 128). Credit returns once the app took a datagram, so 192 capped one QUIC download at 37 Mbit/s on a 50 ms path and dropped every burst beyond it: a phone lost 6.1% of its downlink datagrams (4 076 of 67 035) in an hour of normal use. Bench, phone-like window, 50 ms: a 40 Mbit/s stream lost 15–25% and a 60 Mbit/s one 45–62%; now 0–0.04%. Voice p95 with a parallel UDP and TCP download 70 → 74 ms.
- Agents announce the new limit in `max_udp_slots`; clients stay within the agent's limit, so mixed versions keep working with 192. The Android UDP window setting accepts up to 512 slots.

## 0.3.15 — 2026-11-11

- The TCP budget without a call decides on the delay its own bytes saw in each round trip, weighted by size, instead of the fastest delivery of the round. Ping replies and other light packets pass the writer's round robin at once; with 1 MiB per connection their fast deliveries hid the queue behind bulk flows, and on a phone 12–15% of a speed test's frames waited over 100 ms on the server (loaded ping about 100 ms). The cut threshold is now max(base/4, 50 ms). Bench, 80 Mbit/s and 50 ms: no frame waits over 100 ms (0.3.14: up to 70 per 8 s); 200 ms route, 16 downloads: 2 instead of 814, loaded ping 220 instead of 261 ms; throughput unchanged.
- 0.3.14 (1 MiB window per connection) was not published; its change is part of this release.

## 0.3.14 — 2026-11-11

- One TCP connection may have up to 1 MiB unacknowledged instead of 256 KiB, in both directions. A single connection carries at most `window / RTT`, so one download or upload was capped at 40 Mbit/s on a 50 ms path and 10 Mbit/s on a 200 ms path. Bench: 40 → 154 Mbit/s at 50 ms and 10 → 41 Mbit/s at 200 ms without a rate limit; 39 → 74 Mbit/s on an 80 Mbit/s link with a 50 ms path and the phone's 1 MiB SSH window. Sixteen connections and calls are unchanged. Each flow still grows into the window gradually, and the session budget keeps the queue short.
- The session receive cap is 1 GiB of promised buffer instead of 256 MiB, so 1024 connections still fit; memory is used only while a local app leaves data unread.
- iOS keeps 256 KiB per connection because of the Network Extension memory limit. The window is `tcp_receive_window_bytes` in the SSH tuning (64 KiB–1 MiB).
- Needs the agent from this release for uploads; downloads use the client's window with any elastic agent.

## 0.3.13 — 2026-11-11

- The TCP budget without a call keeps up to max(base/8, 20 ms) of queue and cuts above max(base/4, 40 ms), instead of 10 and 30 ms. With 10 ms a phone's uploads fell to 46–52 Mbit/s in two of three Speedtest runs. On the bench throughput is unchanged and the loaded ping is 97 ms in the phone-like case (82 ms in 0.3.12, 107 ms in 0.3.10). On the phone the uplink is now limited by per-flow windows instead of the budget; download 61–77 Mbit/s, loaded ping 55–101 ms (0.3.10: 77–188).

## 0.3.12 — 2026-11-11

- The session TCP budget follows delivery delay all the time, not only during a call. Once per round trip it compares the fastest `Delivered` of that round with the uncongested delay: it doubles while there is no queue (from 1 MiB), then grows by a quarter; a queue above max(base/4, 30 ms) shrinks it to leave max(base/8, 10 ms). Before, 8 MiB could wait in the agent and the SSH path. 16 downloads, 80 Mbit/s, 50 ms RTT: ping on another connection 107 → 82 ms with a 1 MiB SSH window; 16 uploads 174 → 72 ms; 200 ms route 369 → 268 ms; 10 Mbit/s with a call 164 → 81 ms. Throughput is unchanged or slightly higher.
- After 5 s without an uncongested sample the budget is halved for two round trips so the kept queue drains and the delay estimate stays the path's own; that estimate is the lowest of the last 10–20 s, so a path that became slower is not throttled for the rest of the session. Diagnostics show it as `tcp_flight.path_delay_ms`; Android shows it in the log.
- A flow with at most 32 KiB in flight draws on a separate 512 KiB reserve, so a ping reply, an API call or the start of a connection does not wait for permits behind bulk downloads, also during a call.
- Profiles without a TCP budget (maximum throughput) are unchanged. 0.3.11 was a test build of a stricter variant that cost a phone 25–50% of its speed and was not published.

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
