//! Local diagnostics; measurements use the current OS route, including other VPNs.
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| s == "validate") {
        let raw =
            std::fs::read_to_string(args.get(1).expect("catalog JSON path")).expect("read catalog");
        let catalog = secret_tunnel_hosting_catalog::parse(&raw).expect("valid catalog");
        println!("OK: {} offers", catalog.offers.len());
    } else if args.first().is_some_and(|s| s == "probe") {
        let dir: PathBuf = std::env::temp_dir().join("secret-tunnel-catalog-cli");
        let job = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .to_string();
        let ids = if args.len() > 1 {
            args[1..].to_vec()
        } else {
            vec![
                "ovh".into(),
                "itldc".into(),
                "timeweb".into(),
                "firstvds".into(),
            ]
        };
        let raw = serde_json::json!({"action":"probe","ids":ids,"job":job}).to_string();
        println!(
            "{}",
            secret_tunnel_hosting_catalog::request(&raw, &dir, false)
        );
    } else {
        eprintln!("Usage: catalog validate <json> | catalog probe [offer IDs]");
        std::process::exit(2);
    }
}
