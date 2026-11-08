#[path = "../src/embedded_ssh.rs"]
mod embedded_ssh;
#[path = "../src/udp_protocol.rs"]
mod udp_protocol;
use std::sync::{Arc, Mutex};
fn log_line(_: &Arc<Mutex<Vec<String>>>, message: impl Into<String>) {
    eprintln!("{}", message.into());
}
fn main() {
    let path = std::env::args().nth(1).expect("local config path");
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let server = config["servers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == config["selected_server_id"])
        .unwrap();
    let string = |field: &str| server[field].as_str().unwrap().to_owned();
    let handle = embedded_ssh::start(
        embedded_ssh::EmbeddedServer {
            name: "integration check".into(),
            host: string("host"),
            port: server["port"].as_u64().unwrap() as u16,
            user: string("user"),
            key_path: string("key_path"),
            strict_host_key_checking: "yes".into(),
        },
        embedded_ssh::EmbeddedProxy {
            host: "127.0.0.1".into(),
            port: 1082,
        },
        Arc::new(Mutex::new(Vec::new())),
    )
    .unwrap();
    println!("READY embedded SOCKS 1082");
    std::thread::sleep(std::time::Duration::from_secs(40));
    println!(
        "COUNTERS {:?}, EXIT {:?}",
        handle.traffic_totals(),
        handle.try_wait()
    );
    handle.stop();
}
