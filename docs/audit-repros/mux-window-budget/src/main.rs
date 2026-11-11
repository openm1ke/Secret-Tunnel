//! Independent audit stand: real `secret_tunnel_mux::Endpoint` pairs over an
//! in-memory link with a fixed one-way delay and no rate limit, no loss and no
//! SSH. It isolates three properties of the multiplexer itself:
//!
//!   window  TCP throughput of a flow is bounded by receive window / RTT.
//!   qos     Twelve short datagrams cap all TCP for 15 s only as one voice-like
//!           flow; as DNS lookups on separate flows they no longer do.
//!   budget  Open UDP flows exhaust the session receive budget long before
//!           the advertised flow limit, and then TCP opens are rejected too.
//!
//! Run from the repository root:
//!   CARGO_TARGET_DIR=/tmp/secret-tunnel-audit cargo run --release --locked \
//!     --manifest-path docs/audit-repros/mux-window-budget/Cargo.toml -- all
use secret_tunnel_mux::{Config, Endpoint, FlowKind, FlowWindow, Role, UDP_WINDOW};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, DuplexStream},
    sync::mpsc,
    time::Instant,
};

const CHUNK: usize = 16384;
const QOS: Option<usize> = Some(96 * 1024); // production default (SshTuning)

/// One direction: bytes read at time t are delivered at t + delay, in order.
async fn pipe(
    mut reader: tokio::io::ReadHalf<DuplexStream>,
    mut writer: tokio::io::WriteHalf<DuplexStream>,
    delay: Duration,
) {
    let (tx, mut rx) = mpsc::unbounded_channel::<(Instant, Vec<u8>)>();
    tokio::spawn(async move {
        while let Some((due, data)) = rx.recv().await {
            tokio::time::sleep_until(due).await;
            if writer.write_all(&data).await.is_err() {
                break;
            }
        }
    });
    let mut buffer = vec![0; 65536];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if tx.send((Instant::now() + delay, buffer[..n].to_vec())).is_err() {
                    break;
                }
            }
        }
    }
}

/// A byte link with symmetric one-way delay and unlimited rate.
fn link(delay: Duration) -> (DuplexStream, DuplexStream) {
    let (client, client_far) = tokio::io::duplex(1 << 20);
    let (server, server_far) = tokio::io::duplex(1 << 20);
    let (client_read, client_write) = tokio::io::split(client_far);
    let (server_read, server_write) = tokio::io::split(server_far);
    tokio::spawn(pipe(client_read, server_write, delay));
    tokio::spawn(pipe(server_read, client_write, delay));
    (client, server)
}

async fn endpoints(delay: Duration, client: Config, server: Config) -> (Endpoint, Endpoint) {
    let (a, b) = link(delay);
    let (client, server) = tokio::join!(
        Endpoint::start(a, Role::Client, client),
        Endpoint::start(b, Role::Server, server)
    );
    (client.expect("client mux"), server.expect("server mux"))
}

/// Server side of a download: accept every flow; TCP flows send zeros forever,
/// UDP flows are drained. Flows stay open until the endpoint closes.
fn serve(server: Endpoint) {
    tokio::spawn(async move {
        while let Some(incoming) = server.accept().await {
            tokio::spawn(async move {
                let mut flow = incoming.flow;
                if flow.accept().is_err() {
                    return;
                }
                match incoming.kind {
                    FlowKind::Tcp => {
                        let sender = flow.sender();
                        while sender.send(vec![0; CHUNK]).await.is_ok() {}
                    }
                    FlowKind::Udp => while let Ok(Some(_)) = flow.recv().await {},
                }
            });
        }
    });
}

/// Client side: open `flows` TCP downloads and count delivered payload bytes.
async fn downloads(client: &Endpoint, flows: usize) -> Arc<AtomicU64> {
    let total = Arc::new(AtomicU64::new(0));
    for _ in 0..flows {
        let mut flow = client.open(FlowKind::Tcp, b"download".to_vec()).await.expect("open");
        let total = total.clone();
        tokio::spawn(async move {
            // Dropping each packet returns its credit, as relay() does after write_all.
            while let Ok(Some(packet)) = flow.recv().await {
                total.fetch_add(packet.len() as u64, Ordering::Relaxed);
            }
        });
    }
    total
}

fn mbit(bytes: u64, seconds: f64) -> f64 {
    bytes as f64 * 8.0 / 1e6 / seconds
}

fn config(window: usize) -> Config {
    Config {
        receive_window: window,
        tcp_interactive_bytes: QOS,
        udp_window: Some(UDP_WINDOW),
        ..Default::default()
    }
}

/// Steady-state download rate for a receive window, RTT and flow count.
async fn window_case(window: usize, one_way_ms: u64, flows: usize) -> f64 {
    let delay = Duration::from_millis(one_way_ms);
    // 1 MiB windows need a smaller flow table to fit the 256 MiB session cap.
    let mut cfg = config(window);
    cfg.max_flows = 128;
    let (client, server) = endpoints(delay, cfg.clone(), cfg).await;
    serve(server.clone());
    let total = downloads(&client, flows).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let before = total.load(Ordering::Relaxed);
    tokio::time::sleep(Duration::from_secs(4)).await;
    let rate = mbit(total.load(Ordering::Relaxed) - before, 4.0);
    client.shutdown();
    server.shutdown();
    rate
}

async fn window() {
    println!("## window: TCP download rate, Mbit/s (link: delay only, unlimited rate, no loss)");
    println!("| RTT, ms | flows | 64 KiB (production) | 256 KiB | 1 MiB | 64 KiB / RTT x flows |");
    println!("|---:|---:|---:|---:|---:|---:|");
    for one_way in [10u64, 25, 50, 100] {
        for flows in [1usize, 4] {
            let mut cells = Vec::new();
            for window in [65536usize, 262144, 1048576] {
                cells.push(format!("{:.1}", window_case(window, one_way, flows).await));
            }
            let law = 65536.0 * 8.0 / 1e6 / (one_way as f64 * 2.0 / 1000.0) * flows as f64;
            println!(
                "| {} | {} | {} | {} | {} | {:.1} |",
                one_way * 2,
                flows,
                cells[0],
                cells[1],
                cells[2],
                law
            );
        }
    }
}

/// A SOCKS5 UDP datagram to 192.0.2.1, the form STMUX003 flows carry.
fn datagram(port: u16, payload: usize) -> Vec<u8> {
    let mut data = vec![0, 0, 0, 1, 192, 0, 2, 1];
    data.extend_from_slice(&port.to_be_bytes());
    data.resize(10 + payload, 7);
    data
}

/// 8 downloads over a 50 ms RTT link. At t = 5 s the client sends twelve short
/// datagrams once: as six DNS lookups on six flows, or as one voice-like flow.
async fn qos_case(name: &str, voice: bool) {
    println!("### {name}");
    println!("| second | TCP, Mbit/s | server interactive | effective TCP budget, KiB |");
    println!("|---:|---:|---|---:|");
    let cfg = config(65536);
    let (client, server) = endpoints(Duration::from_millis(25), cfg.clone(), cfg).await;
    serve(server.clone());
    let total = downloads(&client, 8).await;
    let mut flows = Vec::new();
    for _ in 0..if voice { 1 } else { 6 } {
        flows.push(client.open(FlowKind::Udp, Vec::new()).await.expect("udp"));
    }
    let mut last = 0;
    for second in 1..=24u32 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if second == 5 {
            for index in 0..12 {
                let (flow, port) = if voice { (&flows[0], 50_000) } else { (&flows[index % 6], 53) };
                let _ = flow.sender().try_send_datagram(datagram(port, 60));
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        let now = total.load(Ordering::Relaxed);
        let flight = server.snapshot().diagnostics.tcp_flight;
        if matches!(second, 3..=8 | 19..=24) {
            println!(
                "| {second} | {:.1} | {} | {} |",
                mbit(now - last, 1.0),
                flight.interactive,
                flight.effective_bytes / 1024
            );
        }
        last = now;
    }
    drop(flows);
    client.shutdown();
    server.shutdown();
}

async fn qos() {
    println!("## qos: 8 TCP downloads, RTT 50 ms; 12 short datagrams sent once at t=5 s");
    qos_case("six DNS lookups: two datagrams to port 53 on each of six flows", false).await;
    qos_case("voice-like run: twelve datagrams on one flow", true).await;
}

/// Open UDP flows (one per DNS query on Android/iOS, kept by the local adapter
/// until its idle timeout) until the transport rejects one, then try TCP.
async fn budget_case(name: &str, client_cfg: Config, server_cfg: Config) {
    let limit = client_cfg.max_flows.min(server_cfg.max_flows);
    let (client, server) = endpoints(Duration::from_millis(1), client_cfg, server_cfg).await;
    serve(server.clone());
    let mut held = Vec::new();
    let mut error = String::from("none");
    for _ in 0..limit + 8 {
        match client.open(FlowKind::Udp, Vec::new()).await {
            Ok(flow) => held.push(flow),
            Err(reason) => {
                error = reason;
                break;
            }
        }
    }
    let tcp = match client.open(FlowKind::Tcp, b"download".to_vec()).await {
        Ok(_) => "accepted".to_owned(),
        Err(reason) => format!("rejected: {reason}"),
    };
    println!(
        "| {name} | {limit} | {} | {error} | {tcp} | {} / {} |",
        held.len(),
        client.snapshot().rejected,
        server.snapshot().rejected
    );
    drop(held);
    client.shutdown();
    server.shutdown();
}

async fn budget() {
    println!("## budget: idle UDP flows opened before the first rejection");
    println!("| profile | advertised flow limit | UDP flows opened | first error | next TCP open | rejected client / server |");
    println!("|---|---:|---:|---|---|---|");
    let agent = |flows| Config {
        max_flows: flows,
        udp_window: Some(UDP_WINDOW), // --stdio-v3-qos without a client override
        tcp_interactive_bytes: QOS,
        ..Default::default()
    };
    let client = |flows, bytes, slots| Config {
        max_flows: flows,
        udp_window: Some(FlowWindow { bytes, slots }),
        tcp_interactive_bytes: QOS,
        ..Default::default()
    };
    budget_case("Android default (512 flows, UDP 256 KiB/192)", client(512, 262144, 192), agent(2048)).await;
    budget_case("Desktop default (1024 flows, UDP 128 KiB/128)", client(1024, 131072, 128), agent(2048)).await;
    budget_case("iOS as committed (1024 flows, UDP 64 KiB/64)", client(1024, 65536, 64), agent(2048)).await;
    budget_case("iOS first device build (256 flows, UDP 128 KiB/96)", client(256, 131072, 96), agent(2048)).await;
    budget_case("iOS second device build (256 flows, UDP 64 KiB/64)", client(256, 65536, 64), agent(2048)).await;
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    if matches!(mode.as_str(), "all" | "budget") {
        budget().await;
    }
    if matches!(mode.as_str(), "all" | "qos") {
        qos().await;
    }
    if matches!(mode.as_str(), "all" | "window") {
        window().await;
    }
}
