Secret Tunnel v0.3.4 adds SSH server chains on desktop and Android.

- Choose an intermediate SSH server in the destination server profile. Routes support up to three intermediate servers, reject cycles and keep each server's credentials and host-key verification separate. Desktop chains use Embedded Rust SSH. The traffic agent runs only on the final server.
- Android now defaults to a 256 KiB / 192-slot UDP receive window for download and 128 KiB / 128 slots for upload. Explicit per-proxy settings retain priority. An automatic download profile can reduce its slot count to the agent's advertised limit.
- Windows and Android checks passed HTTPS, UDP DNS and association shutdown through the RU → FR chain. A real Android device transferred 8 MiB TCP and all 528 controlled UDP datagrams. During a subsequent LTE run, Instagram, Telegram and Discord worked normally according to the user; the VPN and agent remained active. The tunnel still recorded UDP drops under load. [Measurements and limitations](SSH-CHAIN-MOBILE-2026-11-10.md).
- Existing direct connections and saved application selections are preserved. The bundled Linux agent supports per-session UDP settings; no traffic agent is required on intermediate SSH servers.

Packages include a signed Android APK, Windows setup and portable ZIP, macOS universal DMG and ZIP, and the Linux x86_64 agent archive. macOS per-app routing uses the separately installed ProxyBridge component.
