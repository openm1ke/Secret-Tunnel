//! Real loopback SSH sessions with disposable fixture keys, no user server.
use super::*;
use russh::server::{self, Msg, Session};
use russh::{Channel, ChannelId};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone, Copy)]
enum Mode {
    Current,
    /// Like `Current`, and announces elastic TCP sending.
    ElasticTcp,
    LimitedUdpSlots,
    Relay(u16),
    OldAgent,
    NetworkDrop,
    Missing,
    NoExecute,
    BadJson,
    NoUdp,
    Oversized,
    Hung,
    RejectAuth,
    Upload,
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

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host: &str,
        port: u32,
        _: &str,
        _: u32,
        _: &mut Session,
    ) -> Result<bool, Self::Error> {
        let Mode::Relay(target) = self.mode else { return Ok(false) };
        if port != u32::from(target) || !host.ends_with(".invalid") {
            return Ok(false);
        }
        let Ok(mut socket) = tokio::net::TcpStream::connect(("127.0.0.1", target)).await else {
            return Ok(false);
        };
        self.commands.lock().unwrap().push(format!("forward {host}:{port}"));
        let mut stream = channel.into_stream();
        tokio::spawn(async move { let _ = tokio::io::copy_bidirectional(&mut stream, &mut socket).await; });
        Ok(true)
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
        if matches!(self.mode, Mode::Upload) {
            assert_eq!(command, "fixture upload");
            let mut stream = self.channels.remove(&id).unwrap().into_stream();
            let handle = session.handle();
            tokio::spawn(async move {
                // Force the client to drain output while the server waits to read stdin.
                stream.write_all(&vec![b'x'; 4 * 1024 * 1024]).await.unwrap();
                let mut input = Vec::new();
                stream.read_to_end(&mut input).await.unwrap();
                assert_eq!(input, vec![0xa5; 1024 * 1024]);
                handle.extended_data(id, 1, b"fixture stderr".to_vec()).await.unwrap();
                handle.exit_status_request(id, 7).await.unwrap();
                handle.eof(id).await.unwrap();
                handle.close(id).await.unwrap();
            });
        } else if command == agent::QUERY_COMMAND {
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
                    let current = matches!(self.mode, Mode::Current | Mode::ElasticTcp);
                    caps.flow_control = current || matches!(self.mode, Mode::LimitedUdpSlots);
                    caps.udp_config = current;
                    caps.qos = current;
                    caps.elastic_tcp = matches!(self.mode, Mode::ElasticTcp);
                    if matches!(self.mode, Mode::LimitedUdpSlots) {
                        caps.max_udp_slots = 128;
                    }
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
            // The elastic flag reaches only an agent that announced the capability.
            let flagged = command.strip_suffix(agent::ELASTIC_TCP_FLAG).map(str::trim_end);
            let elastic_tcp = flagged.is_some();
            assert_eq!(elastic_tcp, matches!(self.mode, Mode::ElasticTcp));
            let command = flagged.unwrap_or(command);
            let current = matches!(self.mode, Mode::Current | Mode::ElasticTcp);
            let flow_control = current || matches!(self.mode, Mode::LimitedUdpSlots);
            let qos = current && command == agent::QOS_COMMAND;
            let configured = flow_control && command == agent::UDP_CONFIG_COMMAND;
            assert_eq!(
                command,
                if qos {
                    agent::QOS_COMMAND
                } else if configured {
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
                    let settings = if qos {
                        Some(agent::read_session_settings(&mut stream).await.unwrap())
                    } else {
                        None
                    };
                    let udp_window = if let Some(settings) = settings {
                        Some(settings.udp_window)
                    } else if configured {
                        Some(agent::read_udp_window_request(&mut stream).await.unwrap())
                    } else {
                        flow_control.then_some(secret_tunnel_mux::UDP_WINDOW)
                    };
                    let endpoint = Endpoint::start(
                        stream,
                        Role::Server,
                        MuxConfig {
                            max_flows: 64,
                            receive_window: if elastic_tcp {
                                secret_tunnel_mux::TCP_RECEIVE_WINDOW
                            } else {
                                secret_tunnel_mux::BASE_TCP_WINDOW
                            },
                            udp_window,
                            tcp_interactive_bytes: settings
                                .map(|settings| settings.tcp_interactive_bytes),
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
    fixture_with_cipher(mode, None).await
}
async fn fixture_with_cipher(mode: Mode, cipher: Option<russh::cipher::Name>) -> Fixture {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    // Public fixed seeds are used only by this loopback fixture.
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut host_seed = [44; 32];
    host_seed[..8].copy_from_slice(&id.to_be_bytes());
    let host: ssh_key::PrivateKey = ssh_key::private::Ed25519Keypair::from_seed(&host_seed).into();
    let key: ssh_key::PrivateKey = ssh_key::private::Ed25519Keypair::from_seed(&[45; 32]).into();
    let path = std::env::temp_dir().join(format!(
        "st-preflight-{}-{}.key",
        std::process::id(),
        id
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
    let mut config = server::Config {
        keys: vec![host],
        auth_rejection_time: Duration::ZERO,
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    };
    if let Some(cipher) = cipher {
        config.preferred.cipher = std::borrow::Cow::Owned(vec![cipher]);
    }
    let config = Arc::new(config);
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
async fn remote_command_drains_output_during_upload_and_returns_exit_status() {
    let fixture = fixture(Mode::Upload).await;
    let (status, stdout, stderr) = run_on_server(
        fixture.server.clone(),
        fixture.options.clone(),
        "fixture upload",
        vec![0xa5; 1024 * 1024],
        Duration::from_secs(5),
    ).await.unwrap();
    assert_eq!(status, 7);
    assert_eq!(stdout, "x".repeat(64 * 1024));
    assert_eq!(stderr, "fixture stderr");
}

#[tokio::test]
async fn ssh_chain_forwards_only_ssh_and_runs_tcp_udp_at_the_destination() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut exit = fixture(Mode::Current).await;
        let mut middle = fixture(Mode::Relay(exit.server.port)).await;
        let first = fixture(Mode::Relay(middle.server.port)).await;
        exit.server.host = "exit.invalid".into();
        middle.server.host = "middle.invalid".into();
        exit.options.jumps = [&first, &middle].map(|fixture| EmbeddedJump {
            server: fixture.server.clone(),
            host_key_sha256: fixture.options.host_key_sha256.clone(),
        }).into();
        let route = connect_route(&exit.server, exit.options.clone()).await.unwrap();
        let (transport, _) = start_agent_transport(&route.destination, 64, None, None, false).await.unwrap();
        for kind in [FlowKind::Tcp, FlowKind::Udp] {
            let mut flow = transport.open(kind, vec![]).await.unwrap();
            flow.sender().send(b"through both jumps".to_vec()).await.unwrap();
            assert_eq!(&*flow.recv().await.unwrap().unwrap(), b"through both jumps");
        }
        assert_eq!(*first.commands.lock().unwrap(), [format!("forward middle.invalid:{}", middle.server.port)]);
        assert_eq!(*middle.commands.lock().unwrap(), [format!("forward exit.invalid:{}", exit.server.port)]);
        assert_eq!(*exit.commands.lock().unwrap(), [agent::QUERY_COMMAND, agent::FLOW_CONTROL_COMMAND]);
        transport.shutdown();
        route.disconnect().await;
    }).await.unwrap();
}

#[tokio::test]
async fn a_wrong_jump_fingerprint_prevents_the_forwarding_request() {
    let exit = fixture(Mode::Current).await;
    let relay = fixture(Mode::Relay(exit.server.port)).await;
    let mut options = exit.options.clone();
    options.jumps.push(EmbeddedJump {
        server: relay.server.clone(),
        host_key_sha256: exit.options.host_key_sha256.clone(),
    });
    let error = connect_route(&exit.server, options).await.err().unwrap();
    assert!(error.contains("SSH-звено 1") && error.contains("host key"), "{error}");
    assert!(relay.commands.lock().unwrap().is_empty());
}

#[tokio::test]
async fn forced_cipher_and_small_queue_apply_to_every_ssh_hop() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut exit = fixture_with_cipher(Mode::Current, Some(russh::cipher::AES_128_GCM)).await;
        let relay = fixture_with_cipher(Mode::Relay(exit.server.port), Some(russh::cipher::AES_128_GCM)).await;
        exit.server.host = "aes-exit.invalid".into();
        exit.options.ssh_tuning = SshTuning {
            cipher: CipherPolicy::Aes128Gcm,
            channel_buffer_messages: 8,
            channel_receive_window_bytes: 1024 * 1024,
            tcp_interactive_bytes: None,
            ..Default::default()
        };
        exit.options.jumps.push(EmbeddedJump {
            server: relay.server.clone(),
            host_key_sha256: relay.options.host_key_sha256.clone(),
        });
        let route = connect_route(&exit.server, exit.options.clone())
            .await
            .unwrap();
        let stats = route.outer_tcp.snapshot();
        assert!(stats.connected);
        #[cfg(any(target_os="linux",target_os="android",windows))]
        assert!(stats.rtt_us.is_some(),"{stats:?}");
        let (transport, _) = start_agent_transport(&route.destination,64,None,None,false).await.unwrap();
        let mut udp=transport.open(FlowKind::Udp,vec![]).await.unwrap();
        assert!(udp.sender().try_send_datagram(b"AES chain".to_vec()).unwrap());
        assert_eq!(&*udp.recv().await.unwrap().unwrap(), b"AES chain");
        transport.shutdown();route.disconnect().await;
    }).await.unwrap();
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
                false,
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
async fn automatic_udp_window_uses_agent_limit_but_manual_window_requires_exact_slots() {
    let window = FlowWindow {
        bytes: 256 * 1024,
        slots: 192,
    };
    for automatic in [true, false] {
        let fixture = fixture(Mode::LimitedUdpSlots).await;
        let session = connect_ssh(
            &fixture.server,
            (&*fixture.server.host, fixture.server.port),
            fixture.options.clone(),
        )
        .await
        .unwrap();
        let result = start_agent_transport(&session, 64, Some(window), None, automatic).await;
        if automatic {
            let (transport, _) = result.unwrap();
            let snapshot = transport.snapshot();
            assert_eq!(snapshot.diagnostics.udp_window_bytes, window.bytes);
            assert_eq!(snapshot.diagnostics.udp_window_slots, 128);
            assert_eq!(snapshot.peer_udp_window, Some(secret_tunnel_mux::UDP_WINDOW));
            transport.shutdown();
        } else {
            assert!(result.err().unwrap().starts_with("AGENT_INCOMPATIBLE:"));
            assert_eq!(*fixture.commands.lock().unwrap(), [agent::QUERY_COMMAND]);
        }
    }
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
            let result = start_agent_transport(&session, 64, None, None, false).await;
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
    let error = start_agent_transport(&session, 64, None, None, false)
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

#[tokio::test]
async fn negotiated_qos_budgets_keep_tcp_streaming_with_short_udp_active() {
    tokio::time::timeout(Duration::from_secs(15), async {
        for budget in [65536, 262144] {
            let fixture = fixture(Mode::Current).await;
            let session = connect_ssh(
                &fixture.server,
                (&*fixture.server.host, fixture.server.port),
                fixture.options.clone(),
            )
            .await
            .unwrap();
            let (transport, caps) = start_agent_transport_qos(
                &session,
                64,
                Some(FlowWindow {
                    bytes: 262144,
                    slots: 192,
                }),
                None,
                false,
                Some(budget),
                secret_tunnel_mux::TCP_RECEIVE_WINDOW,
            )
            .await
            .unwrap();
            assert!(caps.qos);
            let mut udp = transport.open(FlowKind::Udp, vec![]).await.unwrap();
            for _ in 0..8 {
                udp.sender().send(vec![1; 160]).await.unwrap();
                drop(udp.recv().await.unwrap().unwrap());
            }
            assert!(transport.snapshot().diagnostics.tcp_flight.interactive);
            assert_eq!(
                transport.snapshot().diagnostics.tcp_flight.configured_bytes,
                Some(budget)
            );
            let mut tcp = transport.open(FlowKind::Tcp, vec![]).await.unwrap();
            let sender = tcp.sender();
            let sending = tokio::spawn(async move {
                for _ in 0..16 {
                    sender.send(vec![42; 16384]).await.unwrap();
                }
                sender.eof().await.unwrap();
            });
            let mut received = 0;
            while let Some(packet) = tcp.recv().await.unwrap() {
                assert!(packet.iter().all(|byte| *byte == 42));
                received += packet.len();
            }
            sending.await.unwrap();
            assert_eq!(received, 262144);
            tcp.finish().await.unwrap();
            assert_eq!(transport.snapshot().diagnostics.tcp_flight.held_bytes, 0);
            transport.shutdown();
            let _ = session
                .disconnect(russh::Disconnect::ByApplication, "", "English")
                .await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn elastic_tcp_windows_need_an_agent_that_announces_them() {
    use secret_tunnel_mux::{BASE_TCP_WINDOW, TCP_RECEIVE_WINDOW};
    tokio::time::timeout(Duration::from_secs(10), async {
        for (mode, window) in [(Mode::ElasticTcp, TCP_RECEIVE_WINDOW), (Mode::Current, BASE_TCP_WINDOW)] {
            let elastic = window == TCP_RECEIVE_WINDOW;
            let fixture = fixture(mode).await;
            let session = connect_ssh(
                &fixture.server,
                (&*fixture.server.host, fixture.server.port),
                fixture.options.clone(),
            )
            .await
            .unwrap();
            let (transport, report) = start_agent_transport(&session, 64, None, None, false)
                .await
                .unwrap();
            assert_eq!(report.elastic_tcp, elastic);
            // Both directions agree: a larger window is never announced to, or
            // by, an end that would fill it at once.
            let snapshot = transport.snapshot();
            assert_eq!(snapshot.tcp_receive_window, window);
            assert_eq!(snapshot.peer_tcp_window, window);
            assert_eq!(
                *fixture.commands.lock().unwrap(),
                [
                    agent::QUERY_COMMAND.to_owned(),
                    agent::command(agent::FLOW_CONTROL_COMMAND, elastic)
                ]
            );
            // More than one window of TCP still round-trips intact through SSH.
            let mut flow = transport.open(FlowKind::Tcp, vec![]).await.unwrap();
            let sender = flow.sender();
            let total = 3 * TCP_RECEIVE_WINDOW;
            let upload = tokio::spawn(async move {
                for index in 0..total / secret_tunnel_mux::TCP_CHUNK {
                    sender
                        .send(vec![index as u8; secret_tunnel_mux::TCP_CHUNK])
                        .await
                        .unwrap();
                }
            });
            let mut received = 0;
            while received < total {
                let packet = flow.recv().await.unwrap().unwrap();
                assert!(packet.iter().all(|byte| *byte == packet[0]));
                received += packet.len();
            }
            upload.await.unwrap();
            assert_eq!(transport.snapshot().protocol_errors, 0);
            transport.shutdown();
            let _ = session
                .disconnect(russh::Disconnect::ByApplication, "", "English")
                .await;
        }
    })
    .await
    .unwrap();
}
