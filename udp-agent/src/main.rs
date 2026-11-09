mod multiplex;
#[path = "../../src-tauri/src/udp_protocol.rs"]
mod udp_protocol;

fn main() -> std::io::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [arg] if arg == "--capabilities" => {
            let caps = secret_tunnel_mux::agent::Capabilities::current(
                env!("CARGO_PKG_VERSION"),
                secret_tunnel_mux::socket_capacity(2048),
            );
            println!("{}", serde_json::to_string(&caps)?);
            Ok(())
        }
        [arg] if arg == "--version" => {
            println!("secret-tunnel-agent {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [arg] if arg == "--stdio-v3" => multiplex::main(),
        [arg] if arg == "--help" => {
            println!("Usage: secret-tunnel-agent --stdio-v3 | --capabilities | --version");
            Ok(())
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Usage: secret-tunnel-agent --stdio-v3 | --capabilities | --version",
        )),
    }
}
