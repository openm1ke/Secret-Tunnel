# Security Policy

Secret Tunnel manages SSH SOCKS proxy profiles and reads the private key path configured by the user.

## Sensitive Data

- Server profiles, local proxy profiles, selected connection, and IPinfo token are stored locally in `~/.config/secret-tunnel/config.json`.
- The application does not copy SSH private keys into its config. It reads the key from the configured filesystem path when starting a tunnel.
- Unauthenticated local SOCKS listeners accept only literal loopback addresses (`127.0.0.1` / `::1`).
- Do not publish screenshots, logs, or config files containing real server IPs, usernames, key paths, or tokens.

## SSH Host Keys

- `System OpenSSH` uses the host key behavior of the installed `ssh` client.
- `Embedded Rust SSH` supports `StrictHostKeyChecking` style behavior:
  - `yes` requires a matching key in `~/.ssh/known_hosts`.
    On a nonstandard port, an existing host-wide entry may match only if no port-specific entry exists; a changed port-specific key is rejected.
  - `accept-new` records the first key and rejects changed keys.
  - `no` accepts any server key and should only be used for local testing.

## macOS application routing

The adapter uses the user's approved, signed ProxyBridge Network Extension. It matches exact application signing identifiers and discovered nested bundle IDs. Standalone Electron tools such as `ShipIt` may share signing identifiers across applications; manual rules for these tools also cover other applications' matching tools. No independent kill switch is provided. See [macOS behavior and validation](docs/MACOS.md).

## Reporting

Please open a private security advisory on GitHub if available, or contact the maintainers privately before publishing exploit details.
