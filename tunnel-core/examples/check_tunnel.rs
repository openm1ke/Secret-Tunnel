//! Isolated integration probe. No server changes; listens on loopback 1082.
use secret_tunnel_core::embedded_ssh::*;
use std::sync::{Arc, Mutex};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(
        args.len(),
        6,
        "host port user key_path SHA256:host_fingerprint"
    );
    let logs = Arc::new(Mutex::new(Vec::new()));
    let tunnel = start_with_options(
        EmbeddedServer {
            name: "probe".into(),
            host: args[1].clone(),
            port: args[2].parse().unwrap(),
            user: args[3].clone(),
            key_path: args[4].clone(),
            strict_host_key_checking: "yes".into(),
        },
        EmbeddedProxy {
            host: "127.0.0.1".into(),
            port: 1082,
        },
        logs.clone(),
        EmbeddedOptions {
            host_key_sha256: Some(args[5].clone()),
            ..Default::default()
        },
    )
    .expect("SSH startup");
    println!("READY: shared core SOCKS TCP/UDP 127.0.0.1:1082");
    for _ in 0..45 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if let Some(error) = tunnel.try_wait() {
            panic!("{error}");
        }
    }
    println!("Traffic totals: {:?}", tunnel.traffic_totals());
    tunnel.stop();
}
