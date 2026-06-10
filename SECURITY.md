# Security Policy

Secret Tunnel manages SSH SOCKS proxy profiles and reads the private key path configured by the user.

## Sensitive Data

- Server profiles, local proxy profiles, selected connection, and IPinfo token are stored locally in `~/.config/secret-tunnel/config.json`.
- The application does not copy SSH private keys into its config. It reads the key from the configured filesystem path when starting a tunnel.
- Do not publish screenshots, logs, or config files containing real server IPs, usernames, key paths, or tokens.

## SSH Host Keys

- `System OpenSSH` uses the host key behavior of the installed `ssh` client.
- `Embedded Rust SSH` supports `StrictHostKeyChecking` style behavior:
  - `yes` requires a matching key in `~/.ssh/known_hosts`.
  - `accept-new` records the first key and rejects changed keys.
  - `no` accepts any server key and should only be used for local testing.

## Reporting

Please open a private security advisory on GitHub if available, or contact the maintainers privately before publishing exploit details.
