# Contributing

Thanks for helping improve Secret Tunnel.

## Development

```bash
npm install
npm run build
cd src-tauri
cargo check
cargo check --features embedded-ssh
```

## Pull requests

- Keep changes focused.
- Do not commit private keys, tokens, server IPs, local configs, or build artifacts.
- Include a short test note in the pull request description.
- Treat `System OpenSSH` as the stable engine and `Embedded Rust SSH` as experimental until it has broader platform testing.
