use super::*;

#[tokio::test]
async fn aborted_open_does_not_wait_for_the_setup_timeout() {
    let (client, server) = pair(Config::default()).await;
    let opening = tokio::spawn(async move { client.open(FlowKind::Tcp, vec![]).await });
    drop(server.accept().await.unwrap());
    let result = tokio::time::timeout(Duration::from_secs(1), opening)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_err());
}

#[tokio::test]
async fn server_banner_waits_for_acceptance_even_with_a_busy_control_queue() {
    let (client, server) = pair(Config {
        max_flows: 4,
        ..Default::default()
    })
    .await;
    let opening = {
        let client = client.clone();
        tokio::spawn(async move { client.open(FlowKind::Tcp, vec![]).await })
    };
    let incoming = server.accept().await.unwrap().flow;
    for _ in 0..128 {
        assert!(server.publish_stats(false));
    }
    incoming.accept().unwrap();
    incoming
        .sender()
        .send(b"server banner".to_vec())
        .await
        .unwrap();
    let mut flow = tokio::time::timeout(Duration::from_secs(2), opening)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(&*flow.recv().await.unwrap().unwrap(), b"server banner");
    assert_eq!(client.snapshot().protocol_errors, 0);
}

#[tokio::test]
async fn slot_credit_is_returned_only_after_application_consumption() {
    let (client, server) = pair(Config {
        max_flows: 4,
        receive_slots: 4,
        ..Default::default()
    })
    .await;
    let opening = {
        let client = client.clone();
        tokio::spawn(async move { client.open(FlowKind::Tcp, vec![]).await.unwrap() })
    };
    let mut incoming = server.accept().await.unwrap().flow;
    incoming.accept().unwrap();
    let flow = opening.await.unwrap();
    let sender = flow.sender();
    for _ in 0..4 {
        sender.send(vec![1]).await.unwrap();
    }
    let pending = tokio::spawn(async move { sender.send(vec![2]).await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(!pending.is_finished());
    let packet = incoming.recv().await.unwrap().unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(!pending.is_finished());
    drop(packet);
    tokio::time::timeout(Duration::from_secs(1), pending)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(server.snapshot().receive_queue_peak <= 4);
}

#[tokio::test]
async fn udp_overload_is_counted_and_does_not_retain_unbounded_packets() {
    let (client, server) = pair(Config {
        max_flows: 4,
        receive_slots: 4,
        ..Default::default()
    })
    .await;
    let opening = {
        let client = client.clone();
        tokio::spawn(async move { client.open(FlowKind::Udp, vec![]).await.unwrap() })
    };
    let mut incoming = server.accept().await.unwrap().flow;
    incoming.accept().unwrap();
    let flow = opening.await.unwrap();
    let sender = flow.sender();
    for _ in 0..4 {
        assert!(sender.try_send_datagram(vec![1]).unwrap());
    }
    assert!(!sender.try_send_datagram(vec![1]).unwrap());
    assert_eq!(client.snapshot().udp_drops, 1);
    assert_eq!(client.snapshot().diagnostics.udp_flow_credit, 1);
    assert_eq!(client.snapshot().diagnostics.udp_credit_slots, 1);
    drop(incoming.recv().await.unwrap().unwrap());
    for _ in 0..100 {
        if sender.state.window.counts.lock().unwrap().slots > 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(sender.try_send_datagram(vec![3]).unwrap());
    assert!(client.snapshot().diagnostics.udp_credit_return.samples > 0);
}

#[tokio::test]
async fn udp_window_is_separate_and_byte_credit_exhaustion_is_reported() {
    let config = Config {
        max_flows: 4,
        udp_window: Some(FlowWindow {
            bytes: 131072,
            slots: 64,
        }),
        ..Default::default()
    };
    let (client, server) = pair(config).await;
    assert_eq!(client.inner.peer_limits.window(FlowKind::Tcp).bytes, 65536);
    let opening = {
        let client = client.clone();
        tokio::spawn(async move { client.open(FlowKind::Udp, vec![]).await.unwrap() })
    };
    let mut receiver = server.accept().await.unwrap().flow;
    receiver.accept().unwrap();
    let flow = opening.await.unwrap();
    let sender = flow.sender();
    for _ in 0..4 {
        assert!(sender.try_send_datagram(vec![7; 32768]).unwrap());
    }
    assert!(!sender.try_send_datagram(vec![7]).unwrap());
    let report = client.snapshot().diagnostics;
    assert_eq!(report.udp_tx_attempts, 5);
    assert_eq!(report.udp_credit_bytes, 1);
    assert_eq!(report.udp_credit_slots, 0);
    let received = receiver.recv().await.unwrap().unwrap();
    assert_eq!(received.len(), 32768);
    assert!(server.snapshot().diagnostics.udp_received > 0);
    assert_eq!(server.snapshot().protocol_errors, 0);
}

#[tokio::test]
async fn larger_udp_windows_share_the_original_session_budget() {
    let (client, server) = pair(Config {
        max_flows: 4,
        udp_window: Some(UDP_WINDOW),
        ..Default::default()
    })
    .await;
    let mut flows = Vec::new();
    for kind in [FlowKind::Udp, FlowKind::Tcp, FlowKind::Tcp] {
        let (opened, accepted) = tokio::join!(client.open(kind, vec![]), async {
            let flow = server.accept().await.unwrap().flow;
            flow.accept().unwrap();
            flow
        });
        flows.push((opened.unwrap(), accepted));
    }
    assert_eq!(server.snapshot().receive_budget, 4 * 65536);
    assert_eq!(client.inner.receive_bytes.available_permits(), 0);
    assert_eq!(
        client.open(FlowKind::Tcp, vec![]).await.err().unwrap(),
        "Receive buffer budget reached"
    );
    drop(flows.pop());
    assert_eq!(client.inner.receive_bytes.available_permits(), 65536);
    let (opened, _) = tokio::join!(client.open(FlowKind::Tcp, vec![]), async {
        let flow = server.accept().await.unwrap().flow;
        flow.accept().unwrap();
        flow
    });
    assert!(opened.is_ok());
}

#[tokio::test]
async fn tagged_probe_and_cancelled_setup_do_not_increment_tcp_errors() {
    let (client, server) = pair(Config {
        udp_window: Some(UDP_WINDOW),
        ..Default::default()
    })
    .await;
    let probe = {
        let client = client.clone();
        tokio::spawn(async move { client.probe_tcp(vec![1]).await })
    };
    let incoming = server.accept().await.unwrap().flow;
    incoming.record_tcp(TcpStage::Connect, Duration::ZERO, Err("unreachable"));
    incoming.reject("unreachable");
    assert!(probe.await.unwrap().is_err());
    for endpoint in [&client, &server] {
        let report = endpoint.snapshot().diagnostics;
        assert_eq!(report.tcp_errors, 0);
        assert_eq!(report.tcp_probes, 1);
        assert_eq!(report.tcp_probe_errors, 1);
        assert!(report.last_tcp.is_none());
    }
    let opening = {
        let client = client.clone();
        tokio::spawn(async move { client.open(FlowKind::Tcp, vec![1]).await })
    };
    drop(server.accept().await.unwrap().flow);
    assert!(opening.await.unwrap().is_err());
    let report = client.snapshot().diagnostics;
    assert_eq!(report.tcp_cancelled, 1);
    assert_eq!(report.tcp_errors, 0);
}

#[tokio::test]
async fn session_statistics_do_not_need_an_application_flow() {
    let (client, server) = pair(Config::default()).await;
    assert!(server.publish_stats(false));
    for _ in 0..100 {
        if client.snapshot().peer.is_some() {
            break;
        }
        tokio::task::yield_now().await;
    }
    let peer = client.snapshot().peer.unwrap();
    assert_eq!(peer.active, 0);
    assert_eq!(peer.limit, 1024);
    assert_eq!(peer.receive_budget, 64 * 1024 * 1024);
    assert!(peer.diagnostics.is_none());
    assert!(Frame {
        kind: Kind::Data,
        id: 0,
        data: vec![1]
    }
    .encode(&mut Vec::new())
    .is_err());
}

#[tokio::test]
async fn diagnostic_report_preserves_remote_error_and_drop_causes() {
    let (client, server) = pair(Config::default()).await;
    let opening = {
        let client = client.clone();
        tokio::spawn(async move { client.open(FlowKind::Tcp, vec![1]).await })
    };
    let incoming = server.accept().await.unwrap().flow;
    let id = incoming.id();
    incoming.record_tcp(
        TcpStage::Connect,
        Duration::from_millis(240),
        Err("refused"),
    );
    incoming
        .sender()
        .note_udp_failure(UdpFailure::Dns, "DNS timeout");
    incoming.reject("refused");
    assert_eq!(opening.await.unwrap().err().unwrap(), "refused");
    assert!(server.publish_stats(true));
    let peer = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(peer) = client.snapshot().peer {
                break peer;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(peer.udp_drops, 1);
    assert_eq!(peer.last_udp_error, "DNS timeout");
    let report = peer.diagnostics.unwrap();
    assert_eq!(report.udp_dns, 1);
    assert_eq!(report.tcp_errors, 1);
    let event = report.last_tcp.unwrap();
    assert_eq!(event.flow, id);
    assert_eq!(event.elapsed_ms, 240);
    assert_eq!(event.outcome, "refused");
    assert_eq!(client.snapshot().diagnostics.last_tcp.unwrap().flow, id);
}

async fn pair(config: Config) -> (Endpoint, Endpoint) {
    let (a, b) = tokio::io::duplex(4096);
    let (client, server) = tokio::join!(
        Endpoint::start(a, Role::Client, config.clone()),
        Endpoint::start(b, Role::Server, config)
    );
    (client.unwrap(), server.unwrap())
}
fn echo_server(endpoint: Endpoint) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                incoming=endpoint.accept()=>{
                    let Some(incoming)=incoming else {break};
                    tasks.spawn(async move {
                        let mut flow=incoming.flow;flow.accept().unwrap();
                        if incoming.target==b"slow" {
                            let mut closed=flow.state.closed.subscribe();
                            if !*closed.borrow(){let _=closed.changed().await;}
                            return;
                        }
                        let sender=flow.sender();
                        while let Ok(Some(packet))=flow.recv().await {sender.send(packet.to_vec()).await.unwrap();}
                        sender.eof().await.unwrap();flow.finish().await.unwrap();
                    });
                }
                Some(result)=tasks.join_next(),if !tasks.is_empty()=>{result.unwrap();}
            }
        }
    })
}

#[tokio::test]
async fn slow_receiver_does_not_block_an_independent_flow() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let config = Config {
            max_flows: 8,
            outbound_bytes: 65536,
            ..Default::default()
        };
        let (client, server) = pair(config).await;
        let serving = echo_server(server.clone());
        let slow = client.open(FlowKind::Tcp, b"slow".to_vec()).await.unwrap();
        let sender = slow.sender();
        let flooding = tokio::spawn(async move {
            for _ in 0..100 {
                sender.send(vec![7; TCP_CHUNK]).await?;
            }
            Ok::<_, String>(())
        });
        for _ in 0..100 {
            if server.snapshot().receive_queue_bytes == 65536 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(!flooding.is_finished());
        let mut fast = client.open(FlowKind::Tcp, b"fast".to_vec()).await.unwrap();
        fast.sender().send(b"independent".to_vec()).await.unwrap();
        fast.sender().eof().await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(&*fast.recv().await.unwrap().unwrap(), b"independent");
        assert!(fast.recv().await.unwrap().is_none());
        fast.finish().await.unwrap();
        drop(slow);
        assert!(flooding.await.unwrap().is_err());
        assert!(server.snapshot().receive_queue_peak <= server.snapshot().receive_budget);
        client.shutdown();
        serving.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn thousand_simultaneous_flows_have_isolated_data_bounded_memory_and_reusable_capacity() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let (client, server) = pair(Config::default()).await;
        let serving = echo_server(server.clone());
        let barrier = Arc::new(tokio::sync::Barrier::new(1025));
        let (start, gate) = watch::channel(false);
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0u32..1024 {
            let endpoint = client.clone();
            let barrier = barrier.clone();
            let mut gate = gate.clone();
            tasks.spawn(async move {
                let mut flow = endpoint
                    .open(FlowKind::Tcp, index.to_be_bytes().to_vec())
                    .await
                    .unwrap();
                let id = flow.id();
                barrier.wait().await;
                while !*gate.borrow() {
                    gate.changed().await.unwrap();
                }
                let data = [index.to_be_bytes().as_slice(), b"payload"].concat();
                flow.sender().send(data.clone()).await.unwrap();
                flow.sender().eof().await.unwrap();
                assert_eq!(&*flow.recv().await.unwrap().unwrap(), data.as_slice());
                assert!(flow.recv().await.unwrap().is_none());
                flow.finish().await.unwrap();
                id
            });
        }
        barrier.wait().await;
        assert_eq!(client.snapshot().active, 1024);
        assert_eq!(server.snapshot().active, 1024);
        assert!(client
            .open(FlowKind::Tcp, b"over limit".to_vec())
            .await
            .is_err());
        start.send(true).unwrap();
        let mut ids = std::collections::HashSet::new();
        while let Some(result) = tasks.join_next().await {
            assert!(ids.insert(result.unwrap()));
        }
        assert_eq!(client.snapshot().active, 0);
        assert_eq!(client.snapshot().peak, 1024);
        let mut extra = client
            .open(FlowKind::Tcp, b"reused capacity".to_vec())
            .await
            .unwrap();
        assert!(!ids.contains(&extra.id()));
        extra.sender().eof().await.unwrap();
        assert!(extra.recv().await.unwrap().is_none());
        extra.finish().await.unwrap();
        for endpoint in [&client, &server] {
            let stats = endpoint.snapshot();
            assert!(stats.receive_queue_peak <= stats.receive_budget);
            assert!(stats.send_queue_peak <= stats.send_budget);
            assert_eq!(stats.protocol_errors, 0);
            assert!(stats.write_batches < stats.frames_tx);
        }
        client.shutdown();
        serving.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn tiny_transport_buffers_and_graceful_close_preserve_all_bytes() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (a, b) = tokio::io::duplex(3);
        let (client, server) = tokio::join!(
            Endpoint::start(a, Role::Client, Config::default()),
            Endpoint::start(b, Role::Server, Config::default())
        );
        let (client, server) = (client.unwrap(), server.unwrap());
        let serving = echo_server(server.clone());
        let mut flow = client
            .open(FlowKind::Tcp, b"fragmented".to_vec())
            .await
            .unwrap();
        let data = vec![91; TCP_CHUNK];
        flow.sender().send(data.clone()).await.unwrap();
        flow.sender().eof().await.unwrap();
        assert_eq!(&*flow.recv().await.unwrap().unwrap(), data.as_slice());
        assert!(flow.recv().await.unwrap().is_none());
        flow.finish().await.unwrap();
        client.shutdown();
        serving.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn excessive_peer_data_is_rejected_before_exceeding_receive_budget() {
    use tokio::io::AsyncWriteExt;
    let (mut attacker, server_io) = tokio::io::duplex(131072);
    let server = tokio::spawn(Endpoint::start(server_io, Role::Server, Config::default()));
    wire::handshake(
        &mut attacker,
        false,
        wire::Limits {
            flows: 1024,
            tcp: FlowWindow {
                bytes: 65536,
                slots: 32,
            },
            udp: None,
        },
    )
    .await
    .unwrap();
    let server = server.await.unwrap().unwrap();
    let mut bytes = Vec::new();
    Frame {
        kind: Kind::OpenTcp,
        id: 1,
        data: b"target".to_vec(),
    }
    .encode(&mut bytes)
    .unwrap();
    attacker.write_all(&bytes).await.unwrap();
    let flow = server.accept().await.unwrap().flow;
    flow.accept().unwrap();
    assert_eq!(
        wire::read(&mut attacker).await.unwrap().kind,
        Kind::Accepted
    );
    bytes.clear();
    for _ in 0..5 {
        Frame {
            kind: Kind::Data,
            id: 1,
            data: vec![1; TCP_CHUNK],
        }
        .encode(&mut bytes)
        .unwrap();
    }
    attacker.write_all(&bytes).await.unwrap();
    for _ in 0..100 {
        if server.error().is_some() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(server.error().unwrap().contains("credit"));
    assert_eq!(server.snapshot().protocol_errors, 1);
    assert_eq!(server.snapshot().receive_queue_peak, 65536);
    drop(flow);
}

#[tokio::test]
async fn late_close_subscriber_observes_cancel_without_another_event() {
    let (client, server) = pair(Config::default()).await;
    let opening = {
        let client = client.clone();
        tokio::spawn(async move { client.open(FlowKind::Tcp, vec![]).await.unwrap() })
    };
    let incoming = server.accept().await.unwrap().flow;
    incoming.accept().unwrap();
    let flow = opening.await.unwrap();
    let sender = incoming.sender();
    drop(incoming);
    assert!(sender.is_closed());
    tokio::time::timeout(Duration::from_secs(1), sender.closed())
        .await
        .unwrap();
    drop(flow);
    client.shutdown();
    tokio::time::timeout(Duration::from_secs(1), client.closed())
        .await
        .unwrap();
}

#[tokio::test]
async fn truncated_v3_frames_and_retired_protocols_are_rejected() {
    use tokio::io::AsyncWriteExt;
    for bytes in [
        vec![0],
        vec![0, 6, 5],
        vec![0, 6, 5, 0, 0, 0, 1],
        vec![0, 4, 5, 0, 0, 0, 1],
    ] {
        assert_eq!(
            wire::read(&mut bytes.as_slice()).await.unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }
    for magic in [b"STUDP001", b"STUDP002"] {
        let (mut peer, mut client) = tokio::io::duplex(64);
        peer.write_all(magic).await.unwrap();
        assert!(wire::handshake(
            &mut client,
            false,
            wire::Limits {
                flows: 64,
                tcp: FlowWindow {
                    bytes: 65536,
                    slots: 32
                },
                udp: None,
            }
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("STMUX003"));
    }
}

#[tokio::test]
async fn abort_reclaims_blocked_relays_and_admission_in_both_directions() {
    use tokio::io::AsyncReadExt;
    for abort in [true, false] {
        let (client, server) = pair(Config {
            max_flows: 4,
            ..Default::default()
        })
        .await;
        for _ in 0..500 {
            let opening = {
                let client = client.clone();
                tokio::spawn(async move { client.open(FlowKind::Tcp, vec![]).await.unwrap() })
            };
            let mut incoming = server.accept().await.unwrap().flow;
            incoming.accept().unwrap();
            let flow = opening.await.unwrap();
            let (mut destination, mut blocked) = tokio::io::duplex(1);
            let mut task =
                tokio::spawn(
                    async move { relay(&mut incoming, &mut destination, |_, _| {}).await },
                );
            flow.sender().send(vec![7; TCP_CHUNK]).await.unwrap();
            // One byte has reached the non-reading destination: write_all is blocked.
            let mut byte = [0];
            blocked.read_exact(&mut byte).await.unwrap();
            assert_eq!(byte, [7]);
            if abort {
                drop(flow)
            } else {
                drop(blocked);
                drop(flow)
            }
            tokio::time::timeout(Duration::from_secs(1), &mut task)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err();
            for _ in 0..100 {
                if server.snapshot().active == 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert_eq!(server.snapshot().active, 0);
            assert_eq!(server.snapshot().receive_queue_bytes, 0);
        }
        // Transport shutdown must also interrupt local socket reads/writes.
        let opening = {
            let client = client.clone();
            tokio::spawn(async move { client.open(FlowKind::Tcp, vec![]).await.unwrap() })
        };
        let mut incoming = server.accept().await.unwrap().flow;
        incoming.accept().unwrap();
        let flow = opening.await.unwrap();
        let (mut destination, _blocked) = tokio::io::duplex(1);
        let task =
            tokio::spawn(async move { relay(&mut incoming, &mut destination, |_, _| {}).await });
        flow.sender().send(vec![7; TCP_CHUNK]).await.unwrap();
        server.shutdown();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        drop(flow);
        client.shutdown();
    }
}

#[tokio::test]
async fn cancelled_relays_close_real_tcp_sockets_in_both_roles() {
    use tokio::{
        io::AsyncReadExt,
        net::{TcpListener, TcpStream},
    };
    tokio::time::timeout(Duration::from_secs(15), async {
        for reverse in [false, true] {
            let (client, server) = pair(Config::default()).await;
            let (opening_side, accepting_side) = if reverse {
                (&server, &client)
            } else {
                (&client, &server)
            };
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            for cycle in 0..100 {
                let opening = {
                    let endpoint = opening_side.clone();
                    tokio::spawn(async move { endpoint.open(FlowKind::Tcp, vec![]).await.unwrap() })
                };
                let mut incoming = accepting_side.accept().await.unwrap().flow;
                incoming.accept().unwrap();
                let flow = opening.await.unwrap();
                let mut destination = TcpStream::connect(listener.local_addr().unwrap())
                    .await
                    .unwrap();
                let (mut receiver, _) = listener.accept().await.unwrap();
                let task =
                    tokio::spawn(
                        async move { relay(&mut incoming, &mut destination, |_, _| {}).await },
                    );
                flow.sender().send(vec![42]).await.unwrap();
                let mut byte = [0];
                receiver.read_exact(&mut byte).await.unwrap();
                assert_eq!(byte, [42]);
                // Both local TCP read and flow receive are now idle. The last cycle
                // also covers transport shutdown rather than an individual Abort.
                if cycle == 99 {
                    accepting_side.shutdown();
                }
                drop(flow);
                tokio::time::timeout(Duration::from_secs(1), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap_err();
                assert_eq!(receiver.read(&mut byte).await.unwrap(), 0);
                assert_eq!(accepting_side.snapshot().active, 0);
                assert_eq!(accepting_side.snapshot().receive_queue_bytes, 0);
            }
            client.shutdown();
            server.shutdown();
        }
    })
    .await
    .unwrap();
}
