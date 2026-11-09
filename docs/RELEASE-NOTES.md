Secret Tunnel v0.3.3 adds per-proxy UDP settings on Android.

- Open a proxy profile's advanced UDP settings to choose preset or custom receive windows for download and upload. Saved settings take effect on the next connection. The dashboard and journal show the applied values.
- Update the Linux agent to v0.3.3 to customize the upload window. The agent validates the requested size and slot count when it starts an SSH session. Each connection has its own windows and receive budget.
- A signed test APK was verified on a realme RMX3630. A controlled run over SSH transferred 8 MiB of TCP data and delivered all 528 UDP datagrams with different windows in the two directions. [Earlier profile benchmarks](PERFORMANCE-0.3.2.md).

Packages include a signed Android APK, Windows setup and portable ZIP, macOS universal DMG and ZIP, and the Linux x86_64 agent archive. macOS per-app routing uses the separately installed ProxyBridge component.
