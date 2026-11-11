mod diagnostic_log;
mod multiplex;
#[path = "../../src-tauri/src/udp_protocol.rs"]
mod udp_protocol;

use multiplex::Mode;

const USAGE: &str = "Usage: secret-tunnel-agent (--stdio-v3 | --stdio-v3-diagnostics | --stdio-v3-flow-control | --stdio-v3-udp-config | --stdio-v3-qos) [--elastic-tcp] | --capabilities | --version";

fn main() -> std::io::Result<()> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    // Sent by clients that grow TCP sending themselves; see `ELASTIC_TCP_FLAG`.
    let elastic_tcp = args.len() == 2 && args[1] == secret_tunnel_mux::agent::ELASTIC_TCP_FLAG;
    if elastic_tcp {
        args.pop();
    }
    match args.as_slice() {
        [arg] if elastic_tcp && !arg.starts_with("--stdio-v3") => {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, USAGE))
        }
        [arg] if arg == "--capabilities" => {
            let mut caps = secret_tunnel_mux::agent::Capabilities::current(
                env!("CARGO_PKG_VERSION"),
                secret_tunnel_mux::socket_capacity(2048),
            );
            caps.diagnostics = true;
            caps.flow_control = true;
            caps.udp_config = true;
            caps.qos = true;
            caps.elastic_tcp = true;
            println!("{}", serde_json::to_string(&caps)?);
            Ok(())
        }
        [arg] if arg == "--version" => {
            println!("secret-tunnel-agent {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [arg] if arg == "--stdio-v3" => multiplex::run(Mode::Basic, elastic_tcp),
        [arg] if arg == "--stdio-v3-diagnostics" => multiplex::run(Mode::Diagnostics, elastic_tcp),
        [arg] if arg == "--stdio-v3-flow-control" => multiplex::run(Mode::FlowControl, elastic_tcp),
        [arg] if arg == "--stdio-v3-udp-config" => multiplex::run(Mode::UdpConfig, elastic_tcp),
        [arg] if arg == "--stdio-v3-qos" => multiplex::run(Mode::Qos, elastic_tcp),
        [arg] if arg == "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, USAGE)),
    }
}
