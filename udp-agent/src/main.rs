mod diagnostic_log;
mod multiplex;
#[path = "../../src-tauri/src/udp_protocol.rs"]
mod udp_protocol;

const USAGE: &str = "Usage: secret-tunnel-agent --stdio-v3 | --stdio-v3-diagnostics | --stdio-v3-flow-control | --capabilities | --version";

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
            println!("{}", serde_json::to_string(&caps)?);
            Ok(())
        }
        [arg] if arg == "--version" => {
            println!("secret-tunnel-agent {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [arg] if arg == "--stdio-v3" => multiplex::run(false, false),
        [arg] if arg == "--stdio-v3-diagnostics" => multiplex::run(true, false),
        [arg] if arg == "--stdio-v3-flow-control" => multiplex::run(true, true),
        [arg] if arg == "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, USAGE)),
    }
}
