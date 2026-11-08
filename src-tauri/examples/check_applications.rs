fn main() {
    match secret_tunnel_lib::application_catalog() {
        Ok(apps) => println!("{}", serde_json::to_string(&apps).unwrap()),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
