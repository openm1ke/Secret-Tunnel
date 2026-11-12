# AGENTS.md — working on Secret Tunnel

For coding agents (Codex, Claude Code, Cursor…) changing this repository.
Helping a *user set the app up* is a different job: read
[docs/SETUP-FOR-AI.md](docs/SETUP-FOR-AI.md) instead.

Secret Tunnel sends the traffic of selected applications through the user's
own SSH server, optionally through a chain of servers. One Rust SSH engine
and one stream multiplexer serve every client; a small agent on the server
carries TCP and UDP inside a single SSH channel.

## Layout

| Path | What it is |
| --- | --- |
| `src/` | Desktop UI: React + TypeScript (Vite). `main.tsx` is the app shell and most tabs; `StatsPanel.tsx`, `SpeedPanel.tsx`, `DomainsPanel.tsx`; shared pieces in `components.tsx`; one stylesheet `styles.css` with design tokens on `:root`. |
| `src-tauri/` | Desktop backend (Tauri 2). `lib.rs`: state, commands, config, run loop. `embedded_ssh.rs`: the embedded SSH engine, SOCKS server, per-app usage, DNS names. `macos_redirect.rs` / `macos_apps.rs`: macOS per-app interception through ProxyBridge and app scanning. `app_redirect.rs`: Windows interception (WinDivert/ProxyBridge). `speedtest.rs`, `stats.rs`, `tray.rs`, `windows_icons.rs`, `icon_png.rs`. |
| `stream-mux/` | `secret-tunnel-mux`: the STMUX003 multiplexer (flows, credit windows, QoS) shared by all clients and the agent. |
| `udp-agent/` | `secret-tunnel-agent`: the server agent (Linux), started over SSH exec. |
| `tunnel-core/` | `secret-tunnel-core`: the shared Rust SSH/SOCKS/UDP code for mobile; it compiles files from `src-tauri/src/` through `#[path]`, so editing those affects Android and iOS too. |
| `android/` | Android app: Kotlin UI and `VpnService` in `app/`, JNI bridge crate in `native/`, hev-socks5-tunnel for TUN. |
| `ios/` | iOS app and Packet Tunnel extension (not part of releases). |
| `routing-engine/`, `session-control/` | Domain rules engine (Windows) and tunnel session state machine. |
| `site/` | GitHub Pages site (landing, knowledge base, screenshots, releases). Published by `scripts/publish-site.sh`. |
| `design/` | Logo, app icon masters, tray icon frames (SVG sources). |
| `docs/` | Design notes, audits, platform guides (mostly Russian). |
| `scripts/` | Builds, packaging, checks, `set-version.py`, `deploy-agent.sh`, `publish-site.sh`. |

## Build and test

Rust stable, Node 20+. From the repository root:

```bash
npm ci && npm run build                         # tsc + vite (desktop UI)
npx tsc --noEmit -p .                           # type-check only
cargo test --manifest-path src-tauri/Cargo.toml --features embedded-ssh --locked
cargo test --manifest-path stream-mux/Cargo.toml --locked
cargo test --manifest-path udp-agent/Cargo.toml --locked
cargo test --manifest-path routing-engine/Cargo.toml --locked
cargo test --manifest-path session-control/Cargo.toml --locked
cargo test --manifest-path tunnel-core/Cargo.toml --locked
cargo check --manifest-path android/native/Cargo.toml --locked
```

Desktop app bundle (macOS): `npm run tauri -- build --bundles app --features embedded-ssh`.
Android APK: `bash scripts/build-android.sh` (JDK 17+, SDK 34, NDK 27.0.12077973); the release
APK needs the signing keystore, so check resources with `./gradlew :app:processDebugResources`.

`--ignored` tests in `speedtest.rs` measure a live tunnel on `127.0.0.1:1080`; run them only on purpose.

## Versions and releases

- Apps and the server agent share **one version**. Change it only with
  `python3 scripts/set-version.py X.Y.Z`; CI and the release workflow run
  `--check` and refuse a tag that any version file does not match.
- Add a `CHANGELOG.md` entry at the top for each release.
- Releasing = commit, push, then push tag `vX.Y.Z`. `.github/workflows/release.yml`
  builds macOS, Windows, Android and the agent and publishes them together.
  `workflow_dispatch` with `publish: false` builds candidates without publishing.
- Update the agent and the apps together; an old agent is rejected
  (`AGENT_INCOMPATIBLE`).

## Things that bit us

- **Timing tests on Windows.** Windows timers are coarse: count elapsed time
  against a deadline, never a number of sleeps (0.3.19's release failed on this).
- **Global state in tests.** `app_usage` and `dns_names` are process-wide; tests
  running in parallel share them, so assert floors, not exact totals.
- **ProxyBridge (macOS).** Rules match a flow by the process's *own* signing
  identifier or file name, not its parent app. The CLI reads its profile once;
  rules change without dropping connections by killing the CLI with SIGKILL
  (the extension keeps its tunnel) and starting a new one (`reload` /
  guardian `handoff`). A plain stop (EOF/SIGTERM) clears the rules.
- **Clash and similar VPNs.** Fake-IP DNS hands apps 198.18.x.x; the engine
  recovers TLS names and sends tunnelled DNS to 1.1.1.1. SSH to the server is
  bound to the physical interface on macOS (`bind_physical_interface`).
- **Speed tests.** Cloudflare rate-limits large downloads (HTTP 429); requests
  stay at 25 MB and fall back to Hetzner. Don't run the full test in a loop.
- **Windows-only code can't be fully built on macOS** (resources need the
  Windows toolchain). Cross-check small parts in a scratch crate with
  `cargo check --target x86_64-pc-windows-msvc`; CI builds the real thing.
- **Tauri sync commands run on the main thread.** Long work belongs in a
  thread or an `async` command.

## Conventions

- UI copy is Russian, plain and specific; keep labels identical across the
  app, `site/knowledge.html` and `docs/SETUP-FOR-AI.md` when renaming.
- Comments say why, not what. Commit messages: a short subject, then the
  reason and the measured effect.
- Never commit private keys, tokens, real server addresses or local configs.
  Screenshots for the site use demo data (documentation IPs 203.0.113.x,
  198.51.100.x).
- User settings live in `~/.config/secret-tunnel/config.json` (macOS/Linux)
  and `%APPDATA%\secret-tunnel\config.json` (Windows); new fields need
  `#[serde(default)]` so older configs keep loading.
