//! Shared commands for checking and updating the agent over a pinned SSH route.

pub fn valid_version(version: &str) -> bool {
    let parts: Vec<_> = version.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && part.parse::<u64>().is_ok()
        })
}

/// Installs the verified archive arriving on stdin in an isolated workspace.
pub fn agent_install_command(version: &str) -> String {
    assert!(valid_version(version));
    format!(
        r#"set -eu
umask 077
d=$(mktemp -d)
trap 'rm -rf -- "$d"' EXIT
trap 'exit 130' HUP INT TERM
cd "$d"
cat > agent.tar.gz
case "$(uname -s):$(uname -m)" in Linux:x86_64|Linux:amd64) ;; *) echo 'This server archive requires Linux x86_64.' >&2; exit 1;; esac
tar xzf agent.tar.gz
[ "$(./secret-tunnel-agent-linux-x86_64 --version)" = 'secret-tunnel-agent {version}' ] || {{ echo 'Agent archive has an unexpected version.' >&2; exit 1; }}
ST_AGENT_INSTALL_DIR="$HOME/.local/bin" sh install-server-agent.sh >install.log 2>&1 || {{ tail -n 20 install.log >&2; exit 1; }}
"$HOME/.local/bin/secret-tunnel-agent" --version"#
    )
}

pub const AGENT_VERSION_COMMAND: &str = r#"agent="$HOME/.local/bin/secret-tunnel-agent"
if [ -e "$agent" ]; then "$agent" --version; else echo 'secret-tunnel-agent нет'; fi"#;

pub fn agent_version_from_output(output: &str) -> Result<Option<&str>, String> {
    output
        .lines()
        .find_map(|line| {
            let value = line.trim().strip_prefix("secret-tunnel-agent ")?;
            if value == "нет" {
                Some(Ok(None))
            } else {
                valid_version(value).then_some(Ok(Some(value)))
            }
        })
        .unwrap_or_else(|| Err("Агент вернул неизвестную версию".into()))
}
