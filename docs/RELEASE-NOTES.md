Secret Tunnel v0.3.2 adds a larger UDP flow window, more detailed transport diagnostics, and Android traffic breakdowns.

- Update both the embedded client and the Linux agent to use the 128 KiB, 128-slot UDP window. TCP and UDP share the existing session receive budget.
- The journal distinguishes UDP byte and slot shortages, reports credit return timing, and separates IPv6 probes and cancellations from TCP failures.
- The Android dashboard shows rates and session totals by app on Android 10 and later. The chart offers 1, 5, and 30 seconds, 1 minute, and the full session with peak timestamps.
- A signed APK upgrade on a realme RMX3630 retained the SSH profile and nine selected apps. Larger UDP bursts still exhausted the byte window during the Instagram trial. See the [phone and controlled benchmark results](PERFORMANCE-0.3.2.md).

Packages include a signed Android APK, Windows setup and portable ZIP, macOS universal DMG and ZIP, and the Linux x86_64 agent archive. macOS per-app routing uses the separately installed ProxyBridge component.
