//! Real loopback SSH sessions with disposable fixture keys, no user server.
use super::*;
use russh::server::{self, Msg, Session};
use russh::{Channel, ChannelId};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone, Copy)]
enum Mode {
    Current,
    OldAgent,
    NetworkDrop,
    Missing,
    NoExecute,
    BadJson,
    NoUdp,
    Oversized,
    Hung,
    RejectAuth,
}
struct Handler {
    mode: Mode,
    key: ssh_key::PublicKey,
    channels: HashMap<ChannelId, Channel<Msg>>,
    commands: Arc<Mutex<Vec<String>>>,
}
impl server::Handler for Handler {
    type Error = russh::Error;
    async fn auth_publickey(
        &mut self,
        user: &str,
        key: &ssh_key::PublicKey,
    ) -> Result<server::Auth, Self::Error> {
        Ok(
            if user == "fixture" && key == &self.key && !matches!(self.mode, Mode::RejectAuth) {
                server::Auth::Accept
            } else {
                server::Auth::reject()
            },
        )
    }
    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        _: &mut Session,
    ) -> Result<bool, Self::Error> {
        // Model a server allowing only one session channel at a time.
        if !self.channels.is_empty() {
            return Ok(false);
        }
        self.channels.insert(channel.id(), channel);
        Ok(true)
    }
    async fn channel_close(&mut self, id: ChannelId, _: &mut Session) -> Result<(), Self::Error> {
        self.channels.remove(&id);
        Ok(())
    }
    async fn exec_request(
        &mut self,
        id: ChannelId,
        command: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = std::str::from_utf8(command).unwrap();
        self.commands.lock().unwrap().push(command.into());
        session.channel_success(id)?;
        if command == agent::QUERY_COMMAND {
            let (exit, stdout, stderr) = match self.mode {
                Mode::Missing => (200, vec![], vec![]),
                Mode::NoExecute => (201, vec![], vec![]),
                Mode::OldAgent => (
                    1,
                    vec![],
                    b"Error: Usage: secret-tunnel-agent [--multiplex]".to_vec(),
                ),
                Mode::BadJson => (0, b"welcome\n{}".to_vec(), vec![]),
                Mode::Oversized => (0, vec![b'x'; agent::OUTPUT_LIMIT + 1], vec![]),
                Mode::Hung => return Ok(()),
                _ => {
                    let mut caps = agent::Capabilities::current("0.1.0", 64);
                    caps.udp = !matches!(self.mode, Mode::NoUdp);
                    caps.flow_control = matches!(self.mode, Mode::Current);
                    caps.udp_config = matches!(self.mode, Mode::Current);
                    (0, serde_json::to_vec(&caps).unwrap(), vec![])
                }
            };
            if !stdout.is_empty() {
                session.data(id, stdout)?;
            }
            if !stderr.is_empty() {
                session.extended_data(id, 1, stderr)?;
            }
            session.exit_status_request(id, exit)?;
            session.eof(id)?;
            session.close(id)?;
        } else {
            let flow_control = matches!(self.mode, Mode::Current);
            let configured = flow_control && command == agent::UDP_CONFIG_COMMAND;
            assert_eq!(
                command,
                if configured {
                    agent::UDP_CONFIG_COMMAND
                } else if flow_control {
                    agent::FLOW_CONTROL_COMMAND
                } else {
                    agent::START_COMMAND
                },
                "No arbitrary remote commands"
            );
            if matches!(self.mode, Mode::NetworkDrop) {
                session.eof(id)?;
                session.close(id)?;
            } else {
                let channel = self.channels.remove(&id).unwrap();
                tokio::spawn(async move {
                    let mut stream = channel.into_stream();
                    let udp_window = if configured {
                        Some(agent::read_udp_window_request(&mut stream).await.unwrap())
                    } else {
                        flow_control.then_some(secret_tunnel_mux::UDP_WINDOW)
                    };
                    let endpoint = Endpoint::start(
                        stream,
                        Role::Server,
                        MuxConfig {
                            max_flows: 64,
                            udp_window,
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
                    while let Some(incoming) = endpoint.accept().await {
                        tokio::spawn(async move {
                            let mut flow = incoming.flow;
                            if incoming.target == b"fixture timeout" {
                                flow.reject("TCP connect timeout");
                                return;
                            }
                            if flow.accept().is_err() {
                                return;
                            }
                            loop {
                                match flow.recv().await {
                                    Ok(Some(data)) => {
                                        if flow.sender().send(data.to_vec()).await.is_err() {
                                            return;
                                        }
                                    }
                                    Ok(None) => break,
                                    Err(_) => return,
                                }
                            }
                            let _ = flow.sender().eof().await;
                            let _ = flow.finish().await;
                        });
                    }
                });
            }
        }
        Ok(())
    }
}
struct Fixture {
    server: EmbeddedServer,
    options: EmbeddedOptions,
    task: tokio::task::JoinHandle<()>,
    path: std::path::PathBuf,
    commands: Arc<Mutex<Vec<String>>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}
async fn fixture(mode: Mode) -> Fixture {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    // Public fixed seeds are used only by this loopback fixture.
    let host: ssh_key::PrivateKey = ssh_key::private::Ed25519Keypair::from_seed(&[44; 32]).into();
    let key: ssh_key::PrivateKey = ssh_key::private::Ed25519Keypair::from_seed(&[45; 32]).into();
    let path = std::env::temp_dir().join(format!(
        "st-preflight-{}-{}.key",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(
        &path,
        key.to_openssh(ssh_key::LineEnding::LF).unwrap().as_bytes(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let options = EmbeddedOptions {
        host_key_sha256: Some(
            host.public_key()
                .fingerprint(ssh_key::HashAlg::Sha256)
                .to_string(),
        ),
        ..Default::default()
    };
    let config = Arc::new(server::Config {
        keys: vec![host],
        auth_rejection_time: Duration::ZERO,
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let commands = Arc::new(Mutex::new(Vec::new()));
    let captured = commands.clone();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        // Only one SSH connection is accepted for query + the real v3 channel.
        drop(listener);
        if let Ok(session) = server::run_stream(
            config,
            socket,
            Handler {
                mode,
                key: key.public_key().clone(),
                channels: HashMap::new(),
                commands: captured,
            },
        )
        .await
        {
            let _ = session.await;
        }
    });
    Fixture {
        server: EmbeddedServer {
            name: "fixture".into(),
            host: "127.0.0.1".into(),
            port,
            user: "fixture".into(),
            key_path: path.to_string_lossy().into_owned(),
            strict_host_key_checking: "yes".into(),
        },
        options,
        task,
        path,
        commands,
    }
}
#[tokio::test]
async fn discovery_and_tcp_udp_data_use_one_authenticated_ssh_connection() {
    tokio::time::timeout(Duration::from_secs(10), async {
        for (udp_receive_window, agent_udp_receive_window) in [
            (None, None),
            (Some(FlowWindow {
                bytes: 192 * 1024,
                slots: 192,
            }), None),
            (Some(FlowWindow {
                bytes: 256 * 1024,
                slots: 192,
            }), Some(FlowWindow {
                bytes: 192 * 1024,
                slots: 160,
            })),
        ] {
            let mode = Mode::Current;
            let fixture = fixture(mode).await;
            let session = connect_ssh(
                &fixture.server,
                (&*fixture.server.host, fixture.server.port),
                fixture.options.clone(),
            )
            .await
            .unwrap();
            let (transport, report) = start_agent_transport(
                &session,
                64,
                udp_receive_window,
                agent_udp_receive_window,
            ).await.unwrap();
            assert_eq!(report.protocols, vec![3]);
            assert!(report.flow_control);
            let window = udp_receive_window.unwrap_or(secret_tunnel_mux::UDP_WINDOW);
            let diagnostics = transport.snapshot().diagnostics;
            assert_eq!(diagnostics.udp_window_bytes, window.bytes);
            assert_eq!(diagnostics.udp_window_slots, window.slots);
            assert_eq!(
                transport.snapshot().peer_udp_window,
                Some(agent_udp_receive_window.unwrap_or(secret_tunnel_mux::UDP_WINDOW))
            );
            for kind in [FlowKind::Tcp, FlowKind::Udp] {
                let mut flow = transport.open(kind, vec![]).await.unwrap();
                flow.sender()
                    .send(b"payload after discovery".to_vec())
                    .await
                    .unwrap();
                let packet = flow.recv().await.unwrap().unwrap();
                assert_eq!(&*packet, b"payload after discovery");
                drop(packet);
                if kind == FlowKind::Tcp {
                    flow.sender().eof().await.unwrap();
                    assert!(flow.recv().await.unwrap().is_none());
                    flow.finish().await.unwrap();
                }
            }
            let error = transport.open(FlowKind::Tcp, b"fixture timeout".to_vec()).await
                .err().expect("Expected the remote rejection");
            assert_eq!(error, "TCP connect timeout");
            assert_eq!(transport.snapshot().diagnostics.last_tcp.unwrap().outcome, error);
            transport.shutdown();
            let _ = session
                .disconnect(russh::Disconnect::ByApplication, "", "English")
                .await;
            assert_eq!(
                *fixture.commands.lock().unwrap(),
                [agent::QUERY_COMMAND, if agent_udp_receive_window.is_some() {
                    agent::UDP_CONFIG_COMMAND
                } else {
                    agent::FLOW_CONTROL_COMMAND
                }]
            );
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn agent_failures_are_distinct_and_do_not_start_the_data_channel() {
    tokio::time::timeout(Duration::from_secs(10), async {
        for (mode, code) in [
            (Mode::Missing, "AGENT_MISSING:"),
            (Mode::NoExecute, "AGENT_NOT_EXECUTABLE:"),
            (Mode::BadJson, "AGENT_INVALID_RESPONSE:"),
            (Mode::NoUdp, "AGENT_INCOMPATIBLE:"),
            (Mode::Oversized, "AGENT_INVALID_RESPONSE:"),
            (Mode::OldAgent, "AGENT_INCOMPATIBLE:"),
        ] {
            let fixture = fixture(mode).await;
            let session = connect_ssh(
                &fixture.server,
                (&*fixture.server.host, fixture.server.port),
                fixture.options.clone(),
            )
            .await
            .unwrap();
            let result = start_agent_transport(&session, 64, None, None).await;
            let error = result.err().expect("Incompatible agent accepted");
            assert!(error.starts_with(code), "{error}");
            assert_eq!(fixture.commands.lock().unwrap().len(), 1);
            let _ = session
                .disconnect(russh::Disconnect::ByApplication, "", "English")
                .await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn failed_authentication_never_runs_agent_commands() {
    let fixture = fixture(Mode::RejectAuth).await;
    let error = connect_ssh(
        &fixture.server,
        (&*fixture.server.host, fixture.server.port),
        fixture.options.clone(),
    )
    .await
    .err()
    .expect("Authentication accepted");
    assert!(error.contains("authentication failed"), "{error}");
    assert!(fixture.commands.lock().unwrap().is_empty());
}
#[tokio::test]
async fn hung_discovery_has_a_deadline() {
    let fixture = fixture(Mode::Hung).await;
    let session = connect_ssh(
        &fixture.server,
        (&*fixture.server.host, fixture.server.port),
        fixture.options.clone(),
    )
    .await
    .unwrap();
    let error = tokio::time::timeout(Duration::from_secs(6), discover_agent(&session))
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.starts_with("AGENT_CHECK_TIMEOUT:"));
    let _ = session
        .disconnect(russh::Disconnect::ByApplication, "", "English")
        .await;
}

#[tokio::test]
async fn network_failure_during_handshake_is_not_an_agent_upgrade_error() {
    let fixture = fixture(Mode::NetworkDrop).await;
    let session = connect_ssh(
        &fixture.server,
        (&*fixture.server.host, fixture.server.port),
        fixture.options.clone(),
    )
    .await
    .unwrap();
    let error = start_agent_transport(&session, 64, None, None)
        .await
        .err()
        .expect("Closed channel accepted");
    assert!(error.starts_with("AGENT_START_FAILED:"), "{error}");
    let _ = session
        .disconnect(russh::Disconnect::ByApplication, "", "English")
        .await;
}

#[tokio::test]
async fn stop_interrupts_discovery_and_releases_the_reserved_socks_port() {
    use std::sync::atomic::AtomicBool;
    let fixture = fixture(Mode::Hung).await;
    let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    let server = fixture.server.clone();
    let options = fixture.options.clone();
    let worker = tokio::task::spawn_blocking(move || {
        start_with_options_cancellable(
            server,
            EmbeddedProxy {
                host: "127.0.0.1".into(),
                port,
            },
            Arc::new(Mutex::new(Vec::new())),
            options,
            &|| signal.load(Ordering::SeqCst),
        )
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.commands.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await
        }
    })
    .await
    .unwrap();
    cancelled.store(true, Ordering::SeqCst);
    let error = tokio::time::timeout(Duration::from_secs(2), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.contains("cancelled"), "{error}");
    assert!(
        TcpListener::bind(("127.0.0.1", port)).is_ok(),
        "Cancelled startup retained the SOCKS port"
    );
}

#[tokio::test]
async fn changed_host_pin_never_runs_agent_commands() {
    let fixture = fixture(Mode::Current).await;
    let mut options = fixture.options.clone();
    options.host_key_sha256 = Some("SHA256:wrong".into());
    assert!(connect_ssh(
        &fixture.server,
        (&*fixture.server.host, fixture.server.port),
        options
    )
    .await
    .is_err());
    assert!(fixture.commands.lock().unwrap().is_empty());
}
