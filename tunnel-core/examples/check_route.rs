//! Isolated TCP/UDP measurement through the shared embedded SSH engine.
use secret_tunnel_core::embedded_ssh::*;
use serde::Deserialize;
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Deserialize)]
struct Server {
    name: String,
    host: String,
    port: u16,
    user: String,
    key_path: String,
    host_key_sha256: String,
}
impl Server {
    fn endpoint(&self) -> EmbeddedServer {
        EmbeddedServer {
            name: self.name.clone(),
            host: self.host.clone(),
            port: self.port,
            user: self.user.clone(),
            key_path: self.key_path.clone(),
            strict_host_key_checking: "yes".into(),
        }
    }
}
#[derive(Deserialize)]
struct Probe {
    server: Server,
    #[serde(default)]
    jumps: Vec<Server>,
    #[serde(default)]
    ssh_tuning: SshTuning,
    udp_receive_window: Option<FlowWindow>,
    socks_port: u16,
    seconds: u64,
}
fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: check_route CONFIG.json");
    let config: Probe =
        serde_json::from_slice(&std::fs::read(path).expect("read config")).expect("probe config");
    assert!((1..=600).contains(&config.seconds));
    config.ssh_tuning.validate().unwrap();
    let logs = Arc::new(Mutex::new(Vec::new()));
    let tunnel = start_with_options(
        config.server.endpoint(),
        EmbeddedProxy {
            host: "127.0.0.1".into(),
            port: config.socks_port,
        },
        logs.clone(),
        EmbeddedOptions {
            host_key_sha256: Some(config.server.host_key_sha256),
            jumps: config
                .jumps
                .iter()
                .map(|hop| EmbeddedJump {
                    server: hop.endpoint(),
                    host_key_sha256: Some(hop.host_key_sha256.clone()),
                })
                .collect(),
            ssh_tuning: config.ssh_tuning,
            udp_receive_window: config.udp_receive_window,
            ..Default::default()
        },
    )
    .expect("SSH startup");
    println!(
        "READY {}",
        json!({"ssh":tunnel.ssh_stats(),"agent":tunnel.agent_summary()})
    );
    for _ in 0..config.seconds {
        std::thread::sleep(std::time::Duration::from_secs(1));
        assert!(tunnel.try_wait().is_none(), "tunnel stopped");
        let stats = tunnel.transport_stats();
        println!(
            "SAMPLE {}",
            json!({"ssh":tunnel.ssh_stats(),"client":stats.diagnostics,
            "server":stats.peer.and_then(|p|p.diagnostics),"send_queue_bytes":stats.send_queue_bytes,
            "receive_queue_bytes":stats.receive_queue_bytes,"udp_drops":stats.udp_drops})
        );
    }
    tunnel.stop();
}
