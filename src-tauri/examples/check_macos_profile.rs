#[cfg(target_os = "macos")]
fn main() {
    let mut args = std::env::args().skip(1);
    let port: u16 = args
        .next()
        .expect("SOCKS port")
        .parse()
        .expect("valid port");
    let ids: Vec<String> = args.collect();
    let profile = secret_tunnel_lib::macos_application_profile(&ids, &[], "127.0.0.1", port, true)
        .expect("valid macOS application profile");
    println!("{}", serde_json::to_string(&profile).unwrap());
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("This diagnostic requires macOS");
    std::process::exit(1);
}
