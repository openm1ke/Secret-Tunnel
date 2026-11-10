Secret Tunnel v0.3.6 reduces delays for short UDP packets during concurrent TCP downloads over Embedded SSH.

- Select **Balanced** (96 KiB, default), **Voice** (64 KiB) or **Maximum speed** (extra TCP budget disabled). Balanced and Voice activate the budget during sustained short UDP traffic. Windows stores the choice in advanced SSH settings, and Android stores it in the proxy profile.
- Full-route heartbeat measures tunnel RTT. Android can reconnect after a network change and shows client and server queue timings in the journal.
- On a chain with 16 TCP downloads and UDP already active, the 96 KiB candidate recorded 204 ms and 484 ms p95 UDP RTT in two shaped scenarios. The unlimited control recorded 2.08 s and 5.36 s. Concurrent startup can still delay early UDP packets. [Measurements and limits](https://github.com/openm1ke/Secret-Tunnel/blob/v0.3.6/docs/QOS-SCHEDULING-2026-11-10.md).
- Tested Windows HTTPS and UDP over SOCKS, HTTPS and UDP through an Android app selected for VPN routing, and two simultaneous clients using independent TCP budgets on the same route.

For Balanced or Voice, install the **server agent from this release** on the final SSH server before reconnecting. Its `--capabilities` response includes `"qos": true`. Select Maximum speed to connect to a compatible STMUX003 agent without this capability. [Agent setup guide](https://github.com/openm1ke/Secret-Tunnel/blob/v0.3.6/docs/SERVER-UDP-SETUP.md).

Packages include a signed Android APK, Windows setup and portable ZIP, macOS universal DMG and ZIP, and the Linux x86_64 agent archive. macOS per-app routing uses the separately installed ProxyBridge component.
