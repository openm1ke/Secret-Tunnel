#!/usr/bin/env sh
set -eu
base=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
case $(uname -m) in x86_64|amd64) binary="$base/secret-tunnel-agent-linux-x86_64";; *) echo 'This archive contains the Linux x86_64 agent; build udp-agent for your architecture.' >&2; exit 1;; esac
mkdir -p "$HOME/.local/bin"
install -m 700 "$binary" "$HOME/.local/bin/secret-tunnel-agent.next"
mv "$HOME/.local/bin/secret-tunnel-agent.next" "$HOME/.local/bin/secret-tunnel-agent"
echo 'Installed user-owned SSH exec agent. No service or public listener was created.'
