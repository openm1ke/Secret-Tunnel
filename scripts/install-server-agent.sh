#!/usr/bin/env sh
set -eu
base=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
fail() { printf '%s\n' "$*" >&2; exit 1; }
digest() { sha256sum -- "$1" | cut -d ' ' -f 1; }
valid_digest() {
    case "$1" in *[!0-9a-f]*|'') return 1;; esac
    [ "${#1}" -eq 64 ]
}
[ "$(uname -s)" = Linux ] || fail 'The server archive requires Linux.'
case $(uname -m) in x86_64|amd64) :;; *) fail 'This server archive requires x86_64.';; esac
dest=${ST_AGENT_INSTALL_DIR:-"$HOME/.local/bin"}
target="$dest/secret-tunnel-agent"
pointer="$dest/.secret-tunnel-agent.previous-sha256"
mode=install
case "${1:-}" in
    --rollback) [ "$#" -eq 1 ] || fail 'Usage: install-server-agent.sh --rollback'; mode=rollback;;
    --sha256) [ "$#" -eq 2 ] || fail 'Usage: install-server-agent.sh --sha256 <trusted digest>'; expected=$2;;
    '') expected=$(cat "$base/secret-tunnel-agent.sha256") || fail 'Missing packaged SHA256';;
    *) fail 'Usage: install-server-agent.sh [--sha256 <digest> | --rollback]';;
esac
umask 077
mkdir -p -- "$dest"
lock="$dest/.secret-tunnel-agent.install-lock"
mkdir -- "$lock" || fail 'Another agent installation is in progress.'
staged= backup_tmp= pointer_tmp=
cleanup() {
    for file in "$staged" "$backup_tmp" "$pointer_tmp"; do
        [ -z "$file" ] || rm -f -- "$file"
    done
    rmdir -- "$lock"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM
[ ! -L "$target" ] && { [ ! -e "$target" ] || [ -f "$target" ]; } || fail 'Agent path is not a regular file.'
staged=$(mktemp "$dest/.secret-tunnel-agent.next.XXXXXX")
if [ "$mode" = rollback ]; then
    [ -f "$pointer" ] && [ ! -L "$pointer" ] || fail 'No rollback record exists.'
    expected=$(cat "$pointer")
    valid_digest "$expected" || fail 'Invalid rollback digest.'
    binary="$target.backup-$expected"
else
    binary="$base/secret-tunnel-agent-linux-x86_64"
fi
valid_digest "$expected" || fail 'Expected a lowercase SHA256 digest.'
[ -f "$binary" ] && [ ! -L "$binary" ] || fail 'Agent source is not a regular file.'
install -m 700 -- "$binary" "$staged"
[ "$(digest "$staged")" = "$expected" ] || fail 'Agent SHA256 mismatch; installed agent is unchanged.'
if [ "$mode" = install ]; then
    python3 - "$staged" <<'PY'
import json, struct, subprocess, sys
binary = sys.argv[1]
with open(binary, "rb") as stream:
    header = stream.read(20)
if len(header) != 20 or header[:6] != b"\x7fELF\x02\x01" or struct.unpack_from("<H", header, 18)[0] != 62:
    raise SystemExit("Expected an x86_64 Linux ELF agent")
caps = json.loads(subprocess.check_output([binary, "--capabilities"], timeout=5))
if not (caps.get("schema") == 1 and caps.get("os") == "linux" and caps.get("arch") == "x86_64"
        and caps.get("protocols") == [3] and caps.get("tcp") is True and caps.get("udp") is True
        and caps.get("max_flows", 0) >= 32):
    raise SystemExit("Agent capabilities do not match STMUX003 requirements")
print("Agent capabilities verified:", caps["agent_version"])
PY
    python3 "$base/test_echo.py" "$staged"
    if [ -f "$target" ]; then
        old=$(digest "$target")
        [ "$old" != "$expected" ] || { echo 'Agent already matches this build.'; exit 0; }
        valid_digest "$old" || fail 'Cannot hash installed agent.'
        backup="$target.backup-$old"
        [ ! -L "$backup" ] || fail 'Rollback path is a symbolic link.'
        if [ ! -e "$backup" ]; then
            backup_tmp=$(mktemp "$dest/.secret-tunnel-agent.backup.XXXXXX")
            install -m 700 -- "$target" "$backup_tmp"
            [ "$(digest "$backup_tmp")" = "$old" ] || fail 'Cannot verify rollback copy.'
            mv -fT -- "$backup_tmp" "$backup"
        fi
        [ -f "$backup" ] && [ "$(digest "$backup")" = "$old" ] || fail 'Rollback copy is invalid.'
        pointer_tmp=$(mktemp "$dest/.secret-tunnel-agent.previous.XXXXXX")
        printf '%s\n' "$old" > "$pointer_tmp"
        mv -fT -- "$pointer_tmp" "$pointer"
    fi
fi
mv -fT -- "$staged" "$target"
printf 'Agent %s complete. Existing sessions retain their running process.\n' "$mode"
