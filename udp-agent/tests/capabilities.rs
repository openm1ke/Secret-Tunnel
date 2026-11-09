use secret_tunnel_mux::agent;
use std::{process::Stdio, time::Duration};

#[tokio::test]
async fn capabilities_exit_without_stdin_eof_and_match_the_shared_contract() {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_secret-tunnel-agent"))
        .arg("--capabilities")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let _open_stdin = child.stdin.take();
    let output = tokio::time::timeout(Duration::from_secs(3), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let caps = agent::parse(Some(0), &output.stdout, &output.stderr).unwrap();
    assert_eq!(caps.protocols, vec![3]);
    assert!(caps.diagnostics);
    assert!(caps.flow_control);
    assert_eq!(caps.agent_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(caps.arch, std::env::consts::ARCH);
    assert!(caps.max_flows >= 32);
}
#[tokio::test]
async fn invalid_v3_arguments_fail_without_opening_the_protocol() {
    for args in [
        vec!["--stdio-v3", "unexpected"],
        vec!["--multiplex"],
        vec![],
    ] {
        let output = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::process::Command::new(env!("CARGO_BIN_EXE_secret-tunnel-agent"))
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}
