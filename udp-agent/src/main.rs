mod diagnostic_log;
mod multiplex;
#[path = "../../src-tauri/src/udp_protocol.rs"]
mod udp_protocol;

use multiplex::Mode;

const USAGE: &str = "Usage: secret-tunnel-agent --stdio-v3 | --stdio-v3-diagnostics | --stdio-v3-flow-control | --stdio-v3-udp-config | --stdio-v3-qos | --capabilities | --version";

fn main() -> std::io::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [arg] if arg == "--capabilities" => {
            let mut caps = secret_tunnel_mux::agent::Capabilities::current(
                env!("CARGO_PKG_VERSION"),
                secret_tunnel_mux::socket_capacity(2048),
            );
            caps.diagnostics = true;
            caps.flow_control = true;
            caps.udp_config = true;
            caps.qos = true;
            println!("{}", serde_json::to_string(&caps)?);
            Ok(())
        }
        [arg] if arg == "--version" => {
            println!("secret-tunnel-agent {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [arg] if arg == "--stdio-v3" => multiplex::run(Mode::Basic),
        [arg] if arg == "--stdio-v3-diagnostics" => multiplex::run(Mode::Diagnostics),
        [arg] if arg == "--stdio-v3-flow-control" => multiplex::run(Mode::FlowControl),
        [arg] if arg == "--stdio-v3-udp-config" => multiplex::run(Mode::UdpConfig),
        [arg] if arg == "--stdio-v3-qos" => multiplex::run(Mode::Qos),
        [arg] if arg == "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, USAGE)),
    }
}
