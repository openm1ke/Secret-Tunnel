//! Isolated integration probe. No server changes; listens on loopback 1082.
use secret_tunnel_core::embedded_ssh::*;
use std::sync::{Arc, Mutex};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert!((6..=7).contains(&args.len()), "host port user key_path SHA256:host_fingerprint [max_flows]");
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
            max_connections: args.get(6).and_then(|value|value.parse().ok()).unwrap_or(1024),
            ..Default::default()
        },
    )
    .expect("SSH startup");
    println!("READY: shared core SOCKS TCP/UDP 127.0.0.1:1082");
    let seconds=std::env::var("SECRET_TUNNEL_PROBE_SECONDS").ok().and_then(|s|s.parse::<u64>().ok()).unwrap_or(45).clamp(10,300);
    for index in 0..seconds {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if let Some(error) = tunnel.try_wait() {
            panic!("{error}");
        }
        if index%5==0&&std::env::var_os("SECRET_TUNNEL_TRACE_METRICS").is_some(){println!("TRANSPORT {:?}",tunnel.transport_stats());}
    }
    println!("Traffic totals: {:?}", tunnel.traffic_totals());
    tunnel.stop();
}
