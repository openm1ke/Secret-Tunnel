//! Exit capabilities are measured inside SSH, independently of the device's
//! own address family. A successful TCP connect says nothing about UDP egress.
use super::udp_protocol::{self, PacketAddress};
use crate::connection_checks::{dns_probe_query, dns_probe_succeeded};
use secret_tunnel_mux::{Endpoint, FlowKind};
use serde::Serialize;
use std::{net::SocketAddr, time::Duration};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct Ipv6Support {
    pub tcp: bool,
    pub udp: bool,
    pub checked_at_ms: u64,
}

impl Ipv6Support {
    /// Android admits the family only when both packet transports work.
    pub fn enabled(self) -> bool {
        self.tcp && self.udp
    }

    pub fn allows(self, kind: FlowKind) -> bool {
        match kind {
            FlowKind::Tcp => self.tcp,
            FlowKind::Udp => self.udp,
        }
    }
}

pub(super) async fn probe(transport: &Endpoint) -> Ipv6Support {
    let tcp = [
        "[2606:4700:4700::1111]:443".parse().unwrap(),
        "[2001:4860:4860::8888]:443".parse().unwrap(),
    ];
    let udp = [
        "[2606:4700:4700::1111]:53".parse().unwrap(),
        "[2001:4860:4860::8888]:53".parse().unwrap(),
    ];
    probe_targets(transport, tcp, udp, Duration::from_secs(3)).await
}

async fn probe_targets(
    transport: &Endpoint,
    tcp: [SocketAddr; 2],
    udp: [SocketAddr; 2],
    limit: Duration,
) -> Ipv6Support {
    let (tcp_a, tcp_b, udp_a, udp_b) = tokio::join!(
        probe_tcp(transport, tcp[0], limit),
        probe_tcp(transport, tcp[1], limit),
        probe_udp(transport, udp[0], limit),
        probe_udp(transport, udp[1], limit),
    );
    Ipv6Support {
        tcp: tcp_a || tcp_b,
        udp: udp_a || udp_b,
        checked_at_ms: secret_tunnel_mux::unix_millis(),
    }
}

async fn probe_tcp(transport: &Endpoint, target: SocketAddr, limit: Duration) -> bool {
    let target = udp_protocol::encode_packet(target, &[]).unwrap();
    matches!(
        tokio::time::timeout(limit, transport.probe_tcp(target)).await,
        Ok(Ok(_))
    )
}

async fn probe_udp(transport: &Endpoint, target: SocketAddr, limit: Duration) -> bool {
    tokio::time::timeout(limit, async {
        let mut flow = transport.open(FlowKind::Udp, vec![]).await.ok()?;
        let query = dns_probe_query(rand::random::<u16>().to_be_bytes());
        flow.sender()
            .send(udp_protocol::encode_packet(target, &query).ok()?)
            .await
            .ok()?;
        while let Ok(Some(packet)) = flow.recv().await {
            if let Ok(reply) = udp_protocol::parse_packet(&packet) {
                if reply.address == PacketAddress::Socket(target)
                    && dns_probe_succeeded(reply.payload, &query)
                {
                    return Some(());
                }
            }
        }
        None
    })
    .await
    .is_ok_and(|result| result.is_some())
}

pub(super) fn allowed(
    support: Option<Ipv6Support>,
    kind: FlowKind,
    address: PacketAddress<'_>,
) -> bool {
    // IPv4-mapped literals still use IPv4 at the exit and need no IPv6 route.
    !matches!(address, PacketAddress::Socket(SocketAddr::V6(address)) if address.ip().to_ipv4_mapped().is_none())
        || support.is_none_or(|support| support.allows(kind))
}

#[cfg(test)]
mod tests {
    use super::*;
    use secret_tunnel_mux::{Config, Role};

    async fn fixture(
        tcp: bool,
        udp: bool,
        noise: bool,
    ) -> (Endpoint, Endpoint, tokio::task::JoinHandle<()>) {
        let (a, b) = tokio::io::duplex(65536);
        let config = Config {
            udp_window: Some(secret_tunnel_mux::UDP_WINDOW),
            ..Default::default()
        };
        let (client, server) = tokio::join!(
            Endpoint::start(a, Role::Client, config.clone()),
            Endpoint::start(b, Role::Server, config)
        );
        let (client, server) = (client.unwrap(), server.unwrap());
        let worker = server.clone();
        let task = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            while let Some(incoming) = worker.accept().await {
                tasks.spawn(async move {
                    let mut flow = incoming.flow;
                    if incoming.kind == FlowKind::Tcp {
                        if tcp {
                            flow.accept().unwrap();
                            let _ = flow.recv().await;
                        } else {
                            flow.reject("fixture: IPv6 TCP unavailable");
                        }
                    } else {
                        flow.accept().unwrap();
                        if let Ok(Some(packet)) = flow.recv().await {
                            let parsed = udp_protocol::parse_packet(&packet).unwrap();
                            let PacketAddress::Socket(target) = parsed.address else {
                                panic!("numeric probe required")
                            };
                            let mut reply = parsed.payload.to_vec();
                            reply[2..4].copy_from_slice(&[0x81, 0x80]);
                            reply[6..8].copy_from_slice(&1u16.to_be_bytes());
                            reply.extend_from_slice(&[
                                0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 30, 0, 4, 203, 0, 113, 1,
                            ]);
                            if noise {
                                let other = SocketAddr::new(target.ip(), target.port() + 1);
                                flow.sender()
                                    .send(udp_protocol::encode_packet(other, &reply).unwrap())
                                    .await
                                    .unwrap();
                                reply[0] ^= 1;
                                flow.sender()
                                    .send(udp_protocol::encode_packet(target, &reply).unwrap())
                                    .await
                                    .unwrap();
                                reply[0] ^= 1;
                            }
                            if udp {
                                flow.sender()
                                    .send(udp_protocol::encode_packet(target, &reply).unwrap())
                                    .await
                                    .unwrap();
                            }
                        }
                        // Hold a blackholed association until the probe's deadline cancels it.
                        let _ = flow.recv().await;
                    }
                });
            }
        });
        (client, server, task)
    }

    fn targets(port: u16) -> [SocketAddr; 2] {
        [
            SocketAddr::new("2001:db8::1".parse().unwrap(), port),
            SocketAddr::new("2001:db8::2".parse().unwrap(), port),
        ]
    }

    #[tokio::test]
    async fn tcp_and_udp_are_verified_independently_and_noise_does_not_pass() {
        for (tcp, udp) in [(true, true), (true, false), (false, true), (false, false)] {
            let (client, server, task) = fixture(tcp, udp, true).await;
            let result = probe_targets(
                &client,
                targets(443),
                targets(53),
                Duration::from_millis(250),
            )
            .await;
            assert_eq!(
                (result.tcp, result.udp, result.enabled()),
                (tcp, udp, tcp && udp)
            );
            assert!(result.checked_at_ms > 0);
            assert_eq!(client.snapshot().active, 0);
            client.shutdown();
            server.shutdown();
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn cancelled_probe_releases_its_flows_without_losing_the_session() {
        let (client, server, task) = fixture(false, false, false).await;
        assert!(tokio::time::timeout(
            Duration::from_millis(20),
            probe_targets(&client, targets(443), targets(53), Duration::from_secs(30))
        )
        .await
        .is_err());
        assert_eq!(client.snapshot().active, 0);
        assert!(client.error().is_none());
        client.shutdown();
        server.shutdown();
        task.await.unwrap();
    }

    #[test]
    fn unavailable_ipv6_never_changes_ipv4_or_hostname_requests() {
        let tcp_only = Some(Ipv6Support {
            tcp: true,
            udp: false,
            checked_at_ms: 1,
        });
        assert!(allowed(
            tcp_only,
            FlowKind::Tcp,
            PacketAddress::Socket("[2001:db8::1]:443".parse().unwrap())
        ));
        assert!(!allowed(
            tcp_only,
            FlowKind::Udp,
            PacketAddress::Socket("[2001:db8::1]:443".parse().unwrap())
        ));
        assert!(allowed(
            tcp_only,
            FlowKind::Udp,
            PacketAddress::Socket("203.0.113.1:443".parse().unwrap())
        ));
        assert!(allowed(
            tcp_only,
            FlowKind::Udp,
            PacketAddress::Socket("[::ffff:203.0.113.1]:443".parse().unwrap())
        ));
        assert!(allowed(
            tcp_only,
            FlowKind::Udp,
            PacketAddress::Domain("example.test", 443)
        ));
    }
}
