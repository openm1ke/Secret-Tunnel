#!/usr/bin/env sh
# Build the Linux x86_64 server agent on this machine and install it on a
# server over SSH, without waiting for the release workflow.
#
#   scripts/deploy-agent.sh user@host [ssh options...]
#   scripts/deploy-agent.sh admin@example.org -p 2222 -i ~/.ssh/id_ed25519
#
# The agent has no C dependencies, so Rust's own linker produces a static
# musl binary on macOS or Linux (`rustup target add x86_64-unknown-linux-musl`).
# The archive is laid out like the release one and installed with
# install-server-agent.sh: SHA256 check, self-test, backup and `--rollback`.
# Running sessions keep their agent until they reconnect.
set -eu
[ "$#" -ge 1 ] || { echo "Usage: $0 user@host [ssh options...]" >&2; exit 2; }
target=$1
shift
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
triple=x86_64-unknown-linux-musl

CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=${CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER:-rust-lld} \
    cargo build --manifest-path "$root/udp-agent/Cargo.toml" --release --target "$triple" --locked
binary="$root/udp-agent/target/$triple/release/secret-tunnel-agent"

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
cp "$binary" "$stage/secret-tunnel-agent-linux-x86_64"
cp "$root/scripts/install-server-agent.sh" "$root/udp-agent/test_echo.py" "$stage/"
shasum -a 256 "$stage/secret-tunnel-agent-linux-x86_64" | cut -d ' ' -f 1 > "$stage/secret-tunnel-agent.sha256"
# macOS tar would add extended attributes that GNU tar warns about.
if tar --version 2>/dev/null | grep -q bsdtar; then
    COPYFILE_DISABLE=1 tar --no-xattrs -czf "$stage.tar.gz" -C "$stage" .
else
    tar -czf "$stage.tar.gz" -C "$stage" .
fi
trap 'rm -rf "$stage" "$stage.tar.gz"' EXIT

# The archive goes through stdin, so scp's different port flag is not needed.
ssh "$@" "$target" 'set -e
dir=$(mktemp -d /tmp/st-agent.XXXXXX)
trap "rm -rf \"$dir\"" EXIT
tar -xzf - -C "$dir"
sh "$dir/install-server-agent.sh"
"$HOME/.local/bin/secret-tunnel-agent" --version' < "$stage.tar.gz"
