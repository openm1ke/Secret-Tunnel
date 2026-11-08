mod legacy;
mod multiplex;
#[path = "../../src-tauri/src/udp_protocol.rs"]
mod udp_protocol;

fn main() -> std::io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--stdio-v3") {
        multiplex::main()
    } else {
        legacy::main()
    }
}
