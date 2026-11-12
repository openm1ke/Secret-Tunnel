use crate::udp_protocol;
use russh::client;
use russh::keys::known_hosts::{check_known_hosts_path, known_host_keys_path, learn_known_hosts};
use russh::keys::{load_secret_key, ssh_key, PrivateKeyWithHashAlg};
use secret_tunnel_mux::agent::{self, Capabilities as AgentCapabilities};
pub use secret_tunnel_mux::{FlowWindow, Snapshot as TransportSnapshot};
use secret_tunnel_mux::{Config as MuxConfig, DatagramBuffers, Endpoint, FlowKind, Role};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, ToSocketAddrs};

#[cfg(test)]
#[path = "agent_preflight_tests.rs"]
mod agent_preflight_tests;
#[path = "tls_target.rs"]
mod tls_target;
#[path = "ssh_transport.rs"]
mod ssh_transport;
pub use ssh_transport::{CipherPolicy, SshSnapshot, SshTuning};
use ssh_transport::{ObservedStream, TcpObserver};

#[derive(Clone, Debug)]
pub struct EmbeddedServer {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub key_path: String,
    pub strict_host_key_checking: String,
}

#[derive(Clone, Debug)]
pub struct EmbeddedProxy {
    pub host: String,
    pub port: u16,
}

pub const MAX_SSH_JUMPS: usize = 3;

#[derive(Clone, Debug)]
pub struct EmbeddedJump {
    pub server: EmbeddedServer,
    pub host_key_sha256: Option<String>,
}

/// Platform integration without changing desktop defaults. Pinning is mandatory
/// in the Android adapter; an empty pin observes the host key but rejects it.
#[derive(Clone, Default)]
pub struct EmbeddedOptions {
    pub host_key_sha256: Option<String>,
    pub observed_host_key: Option<Arc<std::sync::Mutex<Option<String>>>>,
    pub protect_socket: Option<Arc<dyn Fn(i32) -> bool + Send + Sync>>,
    /// Ordered from the first reachable server to the last relay.
    pub jumps: Vec<EmbeddedJump>,
    pub probe_ipv6: bool,
    pub max_connections: usize,
    /// Override the local UDP receive window without changing the peer's window.
    pub udp_receive_window: Option<FlowWindow>,
    /// Allow the automatic local window to use a lower slot limit advertised by the agent.
    pub cap_default_udp_receive_slots: bool,
    pub agent_udp_receive_window: Option<FlowWindow>,
    pub ssh_tuning: SshTuning,
    /// macOS: bind the SSH socket to the primary physical interface, so the
    /// TUN of another VPN (Clash, v2RayTun) does not carry the tunnel itself.
    pub bind_physical_interface: bool,
    /// Filled by the first hop's connect: how the SSH connection was routed.
    pub route_note: Option<Arc<std::sync::Mutex<Option<String>>>>,
}

#[derive(Debug)]
pub struct EmbeddedTunnelHandle {
    stop_tx: tokio::sync::watch::Sender<bool>,
    join: Option<JoinHandle<()>>,
    counters: Arc<TrafficCounters>,
    outcome: Receiver<Result<(), String>>,
    ipv6_supported: Option<bool>,
    transport: Endpoint,
    agent: AgentCapabilities,
    outer_tcp: Arc<TcpObserver>,
    ssh_tuning: SshTuning,
}

impl EmbeddedTunnelHandle {
    pub fn agent_summary(&self) -> String {
        self.agent.summary()
    }
    pub fn transport_stats(&self) -> TransportSnapshot {
        self.transport.snapshot()
    }
    pub fn ssh_stats(&self) -> SshSnapshot {
        SshSnapshot { tuning: self.ssh_tuning.clone(), outer_tcp: self.outer_tcp.snapshot() }
    }
    pub fn ipv6_supported(&self) -> Option<bool> {
        self.ipv6_supported
    }
    pub fn try_wait(&self) -> Option<String> {
        match self.outcome.try_recv() {
            Ok(Err(error)) => Some(error),
            Ok(Ok(())) => Some("Embedded SSH stopped".into()),
            Err(mpsc::TryRecvError::Disconnected) => Some("Embedded SSH worker exited".into()),
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
    pub fn traffic_totals(&self) -> (u64, u64) {
        self.counters.totals()
    }

    pub fn stop(self) {
        let _ = self.stop_with_stats();
    }
    pub fn stop_with_stats(mut self) -> TransportSnapshot {
        let _ = self.stop_tx.send(true);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        self.transport.snapshot()
    }
}

#[derive(Debug, Default)]
struct TrafficCounters {
    rx_total: AtomicU64,
    tx_total: AtomicU64,
}

impl TrafficCounters {
    fn add_rx(&self, bytes: u64) {
        self.rx_total.fetch_add(bytes, Ordering::Relaxed);
    }

    fn add_tx(&self, bytes: u64) {
        self.tx_total.fetch_add(bytes, Ordering::Relaxed);
    }

    fn totals(&self) -> (u64, u64) {
        (
            self.rx_total.load(Ordering::Relaxed),
            self.tx_total.load(Ordering::Relaxed),
        )
    }
}

/// Which application each tunnelled connection belongs to, and how much it
/// carried. The interception engine (ProxyBridge) prints a line per new
/// connection, `TCP Telegram -> 149.154.167.51:443 via SOCKS5 ...`, and then
/// connects to the local SOCKS port itself; the connection to the same
/// destination right after such a line is that application's. Connections
/// without a line come from programs that use the proxy directly.
pub mod app_usage {
    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    /// Connections no interception line accounts for.
    pub const UNATTRIBUTED: &str = "Без перехвата (SOCKS)";
    const ROUTE_TTL: Duration = Duration::from_secs(10);
    const MAX_ROUTES: usize = 4096;

    #[derive(Default)]
    pub struct Counter {
        rx: AtomicU64,
        tx: AtomicU64,
        flows: AtomicU64,
    }

    struct Route {
        at: Instant,
        udp: bool,
        destination: String,
        app: String,
    }

    #[derive(Default)]
    struct Registry {
        routes: VecDeque<Route>,
        apps: HashMap<String, Arc<Counter>>,
    }

    fn registry() -> &'static Mutex<Registry> {
        static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
        REGISTRY.get_or_init(Default::default)
    }

    fn normalize(destination: &str) -> String {
        destination.trim().trim_start_matches('[').replacen("]:", ":", 1).to_ascii_lowercase()
    }

    /// Parse one engine line; lines for direct routes or of other kinds are ignored.
    pub fn note_route(line: &str) {
        let Some((left, right)) = line.split_once(" -> ") else { return };
        if !right.contains("SOCKS") {
            return;
        }
        let Some(destination) = right.split_whitespace().next().filter(|d| !d.contains("unknown")) else { return };
        let words: Vec<&str> = left.split_whitespace().filter(|word| !(word.starts_with('[') && word.ends_with(']'))).collect();
        let protocol = words.iter().position(|word| word.eq_ignore_ascii_case("TCP") || word.eq_ignore_ascii_case("UDP"));
        let udp = protocol.is_some_and(|index| words[index].eq_ignore_ascii_case("UDP"));
        let app = match protocol {
            Some(index) => words[index + 1..].join(" "),
            None => words.join(" "),
        };
        // Windows lines may append the process id: `Discord.exe (1234)`.
        // Only a numeric suffix is a pid; "Codex (Renderer)" keeps its name.
        let app = app.trim();
        let app = match app.rsplit_once(" (") {
            Some((name, pid)) if pid.strip_suffix(')').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) => name,
            _ => app,
        }
        .trim()
        .to_owned();
        if app.is_empty() {
            return;
        }
        let mut registry = registry().lock().unwrap();
        let now = Instant::now();
        while registry.routes.front().is_some_and(|route| now.duration_since(route.at) > ROUTE_TTL) || registry.routes.len() >= MAX_ROUTES {
            registry.routes.pop_front();
        }
        registry.routes.push_back(Route { at: now, udp, destination: normalize(destination), app });
    }

    /// The application of the oldest unmatched line for this destination.
    fn claim(udp: bool, destination: &str) -> Option<String> {
        let destination = normalize(destination);
        let mut registry = registry().lock().unwrap();
        let index = registry.routes.iter().position(|route| route.udp == udp && route.destination == destination)?;
        registry.routes.remove(index).map(|route| route.app)
    }

    fn counter(app: &str) -> Arc<Counter> {
        registry().lock().unwrap().apps.entry(app.to_owned()).or_default().clone()
    }

    /// Start counting a new tunnel session from zero.
    pub fn reset() {
        let mut registry = registry().lock().unwrap();
        registry.apps.clear();
        registry.routes.clear();
    }

    /// Bytes received and sent and connections per application this session.
    pub fn snapshot() -> Vec<(String, u64, u64, u64)> {
        registry().lock().unwrap().apps.iter()
            .map(|(name, c)| (name.clone(), c.rx.load(Ordering::Relaxed), c.tx.load(Ordering::Relaxed), c.flows.load(Ordering::Relaxed)))
            .collect()
    }

    /// One connection's bytes. The engine may print its line a little after
    /// it connects, so bytes wait here until the application is known.
    pub struct FlowUsage {
        app: OnceLock<Arc<Counter>>,
        rx: AtomicU64,
        tx: AtomicU64,
    }

    impl FlowUsage {
        pub fn start(udp: bool, destination: &str) -> Arc<Self> {
            let flow = Arc::new(Self { app: OnceLock::new(), rx: AtomicU64::new(0), tx: AtomicU64::new(0) });
            if let Some(app) = claim(udp, destination) {
                flow.bind(&app);
            } else if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let pending = flow.clone();
                let destination = destination.to_owned();
                runtime.spawn(async move {
                    for _ in 0..30 {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        if let Some(app) = claim(udp, &destination) {
                            pending.bind(&app);
                            return;
                        }
                    }
                    pending.bind(UNATTRIBUTED);
                });
            } else {
                flow.bind(UNATTRIBUTED);
            }
            flow
        }

        fn bind(&self, app: &str) {
            let counter = counter(app);
            counter.flows.fetch_add(1, Ordering::Relaxed);
            if self.app.set(counter.clone()).is_ok() {
                counter.rx.fetch_add(self.rx.swap(0, Ordering::Relaxed), Ordering::Relaxed);
                counter.tx.fetch_add(self.tx.swap(0, Ordering::Relaxed), Ordering::Relaxed);
            }
        }

        pub fn add_rx(&self, bytes: u64) {
            match self.app.get() {
                Some(counter) => counter.rx.fetch_add(bytes, Ordering::Relaxed),
                None => self.rx.fetch_add(bytes, Ordering::Relaxed),
            };
        }

        pub fn add_tx(&self, bytes: u64) {
            match self.app.get() {
                Some(counter) => counter.tx.fetch_add(bytes, Ordering::Relaxed),
                None => self.tx.fetch_add(bytes, Ordering::Relaxed),
            };
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn connections_are_attributed_to_the_engine_lines() {
            reset();
            note_route("[11:13:48] TCP Telegram -> 149.154.167.51:443 via SOCKS5 127.0.0.1:1080");
            note_route("[11:13:48] UDP Discord Helper (Renderer) -> [2606:4700::1]:50001 via SOCKS5");
            note_route("[11:13:48] UDP DirectProbe -> unknown:unknown via Direct");
            note_route("[CONN] TCP Discord.exe (4321) -> 1.2.3.4:443 via SOCKS5 proxy");
            let telegram = FlowUsage::start(false, "149.154.167.51:443");
            telegram.add_rx(1000);
            telegram.add_tx(10);
            FlowUsage::start(true, "[2606:4700::1]:50001").add_rx(300);
            FlowUsage::start(false, "1.2.3.4:443").add_tx(7);
            // No line: counted after the wait as a direct proxy client.
            let browser = FlowUsage::start(false, "example.org:443");
            browser.add_rx(50);
            tokio::time::sleep(Duration::from_millis(1700)).await;
            browser.add_rx(5);
            // Other tests relay through SOCKS concurrently and add to the
            // shared unattributed counter, so that one is checked as a floor.
            let mut usage = snapshot();
            usage.sort();
            let unattributed = usage.iter().find(|entry| entry.0 == UNATTRIBUTED).unwrap().1;
            assert!(unattributed >= 55, "unattributed rx {unattributed}");
            usage.retain(|entry| entry.0 != UNATTRIBUTED);
            assert_eq!(usage, vec![
                ("Discord Helper (Renderer)".to_owned(), 300, 0, 1),
                ("Discord.exe".to_owned(), 0, 7, 1),
                ("Telegram".to_owned(), 1000, 10, 1),
            ]);
        }
    }
}

struct SshClient {
    host: String,
    port: u16,
    strict_host_key_checking: String,
    options: EmbeddedOptions,
}

// Like OpenSSH, use an existing host-wide trust entry on a nonstandard port
// only when there is no port-specific entry. A changed scoped key still fails.
fn check_desktop_host_key_at(
    host: &str,
    port: u16,
    key: &ssh_key::PublicKey,
    path: &std::path::Path,
) -> Result<bool, russh::keys::Error> {
    let scoped = known_host_keys_path(host, port, path)?;
    let trust_port = if port != 22 && scoped.is_empty() {
        22
    } else {
        port
    };
    check_known_hosts_path(host, trust_port, key, path)
}

fn check_desktop_host_key(
    host: &str,
    port: u16,
    key: &ssh_key::PublicKey,
) -> Result<bool, russh::keys::Error> {
    let path = std::env::home_dir()
        .ok_or(russh::keys::Error::NoHomeDir)?
        .join(".ssh/known_hosts");
    check_desktop_host_key_at(host, port, key, &path)
}

impl client::Handler for SshClient {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        let fingerprint = server_public_key
            .fingerprint(ssh_key::HashAlg::Sha256)
            .to_string();
        if let Some(observed) = &self.options.observed_host_key {
            if let Ok(mut observed) = observed.lock() {
                *observed = Some(fingerprint.clone());
            }
        }
        if let Some(expected) = &self.options.host_key_sha256 {
            return Ok(!expected.is_empty() && expected == &fingerprint);
        }
        match self.strict_host_key_checking.as_str() {
            "no" => Ok(true),
            "accept-new" => {
                match check_desktop_host_key(&self.host, self.port, server_public_key) {
                    Ok(true) => Ok(true),
                    Ok(false) => {
                        Ok(learn_known_hosts(&self.host, self.port, server_public_key).is_ok())
                    }
                    Err(_) => Ok(false),
                }
            }
            _ => Ok(
                check_desktop_host_key(&self.host, self.port, server_public_key).unwrap_or(false),
            ),
        }
    }
}

struct SshRoute {
    destination: client::Handle<SshClient>,
    jumps: Vec<client::Handle<SshClient>>,
    outer_tcp: Arc<TcpObserver>,
}

impl SshRoute {
    async fn disconnect(&self) {
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            for session in std::iter::once(&self.destination).chain(self.jumps.iter().rev()) {
                let _ = session
                    .disconnect(russh::Disconnect::ByApplication, "", "English")
                    .await;
            }
        })
        .await;
    }
}

#[cfg(test)]
mod platform_tests {
    use super::*;
    #[test]
    fn cancellation_interrupts_a_stalled_ssh_handshake_and_closes_the_socket() {
        use std::io::Read;
        use std::sync::atomic::AtomicBool;
        use std::time::Instant;
        // Deliberately public fixture seed: never a user credential or trusted key.
        let key: ssh_key::PrivateKey =
            ssh_key::private::Ed25519Keypair::from_seed(&[23; 32]).into();
        let path = std::env::temp_dir().join(format!("st-cancel-test-{}.key", std::process::id()));
        std::fs::write(
            &path,
            key.to_openssh(ssh_key::LineEnding::LF).unwrap().as_bytes(),
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let key_path = path.to_string_lossy().into_owned();
        let worker = thread::spawn(move || {
            start_cancellable(
                EmbeddedServer {
                    name: "fixture".into(),
                    host: "127.0.0.1".into(),
                    port,
                    user: "fixture".into(),
                    key_path,
                    strict_host_key_checking: "yes".into(),
                },
                EmbeddedProxy {
                    host: "127.0.0.1".into(),
                    port: 0,
                },
                Arc::new(std::sync::Mutex::new(Vec::new())),
                &|| signal.load(Ordering::SeqCst),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let accepted = loop {
            match listener.accept() {
                Ok((socket, _)) => break Some(socket),
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(5))
                }
                _ => break None,
            }
        };
        if accepted.is_none() {
            cancelled.store(true, Ordering::SeqCst);
            let result = worker.join();
            let _ = std::fs::remove_file(&path);
            panic!("Client did not reach the fixture: {result:?}");
        }
        let mut socket = accepted.unwrap();
        // Windows can inherit the listener's nonblocking mode on accept.
        // The fixture uses blocking reads with a deadline below.
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        // Observe the client identification; intentionally never send a server banner.
        let mut byte = [0];
        socket.read_exact(&mut byte).unwrap();
        let started = Instant::now();
        cancelled.store(true, Ordering::SeqCst);
        let result = worker.join().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(result.unwrap_err().contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(2));
        let mut remaining = Vec::new();
        socket.read_to_end(&mut remaining).unwrap(); // Must reach EOF, not a read timeout.
    }
    #[test]
    fn host_wide_trust_never_overrides_a_changed_port_specific_key() {
        let make_key = |byte| {
            let mut wire = Vec::new();
            wire.extend_from_slice(&11u32.to_be_bytes());
            wire.extend_from_slice(b"ssh-ed25519");
            wire.extend_from_slice(&32u32.to_be_bytes());
            wire.extend_from_slice(&[byte; 32]);
            ssh_key::PublicKey::from_bytes(&wire).unwrap()
        };
        let key = make_key(7);
        let changed = make_key(8);
        let path = std::env::temp_dir().join(format!("secret-tunnel-trust-{}", std::process::id()));
        std::fs::write(
            &path,
            format!("example.test {}\n", key.to_openssh().unwrap()),
        )
        .unwrap();
        assert!(check_desktop_host_key_at("example.test", 2222, &key, &path).unwrap());
        assert!(check_desktop_host_key_at("example.test", 2222, &changed, &path).is_err());
        std::fs::write(
            &path,
            format!(
                "example.test {}\n[example.test]:2222 {}\n",
                key.to_openssh().unwrap(),
                changed.to_openssh().unwrap()
            ),
        )
        .unwrap();
        assert!(check_desktop_host_key_at("example.test", 2222, &key, &path).is_err());
        assert!(check_desktop_host_key_at("example.test", 2222, &changed, &path).unwrap());
        assert!(!check_desktop_host_key_at("unknown.test", 2222, &key, &path).unwrap());
        std::fs::remove_file(path).unwrap();
    }
    #[tokio::test]
    async fn explicit_host_pin_never_falls_back_to_accept_all() {
        let mut wire = Vec::new();
        wire.extend_from_slice(&11u32.to_be_bytes());
        wire.extend_from_slice(b"ssh-ed25519");
        wire.extend_from_slice(&32u32.to_be_bytes());
        wire.extend_from_slice(&[7u8; 32]);
        let key = ssh_key::PublicKey::from_bytes(&wire).unwrap();
        let fingerprint = key.fingerprint(ssh_key::HashAlg::Sha256).to_string();
        let observed = Arc::new(std::sync::Mutex::new(None));
        let mut handler = SshClient {
            host: "example.test".into(),
            port: 22,
            strict_host_key_checking: "no".into(),
            options: EmbeddedOptions {
                host_key_sha256: Some(String::new()),
                observed_host_key: Some(observed.clone()),
                ..Default::default()
            },
        };
        assert!(!client::Handler::check_server_key(&mut handler, &key)
            .await
            .unwrap());
        assert_eq!(*observed.lock().unwrap(), Some(fingerprint.clone()));
        handler.options.host_key_sha256 = Some(fingerprint);
        assert!(client::Handler::check_server_key(&mut handler, &key)
            .await
            .unwrap());
        handler.options.host_key_sha256 = Some("SHA256:wrong".into());
        assert!(!client::Handler::check_server_key(&mut handler, &key)
            .await
            .unwrap());
    }
}

fn friendly_key_error(key_path: &str, error: impl std::fmt::Display) -> String {
    let raw = error.to_string();
    let lower = raw.to_lowercase();
    if lower.contains("encrypted") || lower.contains("passphrase") {
        return format!(
            "Embedded SSH cannot open encrypted private key '{key_path}' yet. Use System OpenSSH or a dedicated key without passphrase for embedded mode."
        );
    }
    if lower.contains("no such file") || lower.contains("not found") {
        return format!("SSH key file not found: {key_path}");
    }
    format!("Cannot load SSH key for embedded engine '{key_path}': {raw}")
}

fn friendly_connect_error(server: &EmbeddedServer, error: impl std::fmt::Display) -> String {
    let raw = error.to_string();
    let lower = raw.to_lowercase();
    if lower.contains("key") && lower.contains("reject")
        || lower.contains("unknownkey")
        || lower.contains("unknown server key")
    {
        return format!(
            "Embedded SSH rejected the server host key for {}:{}. Verify the server fingerprint and update the trusted host key before reconnecting.",
            server.host, server.port
        );
    }
    if lower.contains("timed out") || lower.contains("timeout") {
        return format!(
            "Embedded SSH timed out connecting to {}:{}. Check host, port, firewall, and network reachability.",
            server.host, server.port
        );
    }
    if lower.contains("connection refused") {
        return format!(
            "Embedded SSH connection refused by {}:{}. Check the SSH port and server firewall.",
            server.host, server.port
        );
    }
    format!(
        "Embedded SSH connect failed for '{}' at {}:{}: {raw}",
        server.name, server.host, server.port
    )
}

fn friendly_auth_error(server: &EmbeddedServer, error: impl std::fmt::Display) -> String {
    let raw = error.to_string();
    format!(
        "Embedded SSH public key authentication failed for {}@{}: {raw}. Check username, key path, authorized_keys, and key permissions.",
        server.user, server.host
    )
}

pub fn start(
    server: EmbeddedServer,
    proxy: EmbeddedProxy,
    logs: Arc<std::sync::Mutex<Vec<String>>>,
) -> Result<EmbeddedTunnelHandle, String> {
    start_with_options(server, proxy, logs, EmbeddedOptions::default())
}

pub fn start_with_options(
    server: EmbeddedServer,
    proxy: EmbeddedProxy,
    logs: Arc<std::sync::Mutex<Vec<String>>>,
    options: EmbeddedOptions,
) -> Result<EmbeddedTunnelHandle, String> {
    start_with_options_cancellable(server, proxy, logs, options, &|| false)
}

pub fn start_cancellable(
    server: EmbeddedServer,
    proxy: EmbeddedProxy,
    logs: Arc<std::sync::Mutex<Vec<String>>>,
    cancelled: &dyn Fn() -> bool,
) -> Result<EmbeddedTunnelHandle, String> {
    start_with_options_cancellable(server, proxy, logs, EmbeddedOptions::default(), cancelled)
}

pub fn start_with_options_cancellable(
    server: EmbeddedServer,
    proxy: EmbeddedProxy,
    logs: Arc<std::sync::Mutex<Vec<String>>>,
    options: EmbeddedOptions,
    cancelled: &dyn Fn() -> bool,
) -> Result<EmbeddedTunnelHandle, String> {
    options.ssh_tuning.validate()?;
    if !proxy
        .host
        .parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
    {
        return Err("Embedded SOCKS currently requires a loopback bind address".into());
    }
    let listener = TcpListener::bind((proxy.host.as_str(), proxy.port)).map_err(|error| {
        format!(
            "Cannot bind embedded SOCKS listener on {}:{}: {error}",
            proxy.host, proxy.port
        )
    })?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;

    let counters = Arc::new(TrafficCounters::default());
    let tunnel_counters = Arc::clone(&counters);
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let (ready_tx, ready_rx) = mpsc::channel();
    let (outcome_tx, outcome) = mpsc::channel();
    let join = thread::spawn(move || {
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        if options.protect_socket.is_some() {
            builder.worker_threads(2);
        }
        let runtime = match builder.enable_all().build() {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = outcome_tx.send(Err(error.to_string()));
                return;
            }
        };

        let mut interrupted = stop_rx.clone();
        let result = runtime.block_on(async {
            tokio::select! {
                biased;
                _ = async {if !*interrupted.borrow(){let _=interrupted.changed().await;}} => Ok(()),
                result=run_embedded_tunnel(listener,server,stop_rx,tunnel_counters,ready_tx,logs,options) => result,
            }
        });
        let _ = outcome_tx.send(result);
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    let readiness = loop {
        if cancelled() {
            break Err(mpsc::RecvTimeoutError::Disconnected);
        }
        match ready_rx.recv_timeout(Duration::from_millis(50)) {
            Err(mpsc::RecvTimeoutError::Timeout) if std::time::Instant::now() < deadline => {
                continue
            }
            result => break result,
        }
    };
    if readiness.is_err() {
        let _ = stop_tx.send(true);
        let _ = join.join();
        if cancelled() {
            return Err("Tunnel startup cancelled".into());
        }
        if matches!(readiness, Err(mpsc::RecvTimeoutError::Timeout)) {
            return Err("Embedded SSH/agent startup timed out after 45 seconds".into());
        }
        return Err(outcome
            .try_recv()
            .ok()
            .and_then(Result::err)
            .unwrap_or_else(|| "Embedded SSH startup failed".into()));
    }

    let (ipv6_supported, transport, agent, outer_tcp, ssh_tuning) = readiness.unwrap();
    Ok(EmbeddedTunnelHandle {
        stop_tx,
        join: Some(join),
        counters,
        outcome,
        ipv6_supported,
        transport,
        agent,
        outer_tcp,
        ssh_tuning,
    })
}

async fn run_embedded_tunnel(
    listener: TcpListener,
    server: EmbeddedServer,
    mut stop: tokio::sync::watch::Receiver<bool>,
    counters: Arc<TrafficCounters>,
    ready: Sender<(Option<bool>, Endpoint, AgentCapabilities, Arc<TcpObserver>, SshTuning)>,
    logs: Arc<std::sync::Mutex<Vec<String>>>,
    options: EmbeddedOptions,
) -> Result<(), String> {
    let listener =
        tokio::net::TcpListener::from_std(listener).map_err(|error| error.to_string())?;
    let probe_ipv6 = options.probe_ipv6;
    let log_tcp = options.protect_socket.is_none();
    let ssh_tuning = options.ssh_tuning.clone();
    let udp_receive_window = options.udp_receive_window;
    let cap_default_udp_receive_slots = options.cap_default_udp_receive_slots;
    let agent_udp_receive_window = options.agent_udp_receive_window;
    let requested = if options.max_connections == 0 {
        1024
    } else {
        options.max_connections.clamp(64, 4096)
    };
    let max_connections = secret_tunnel_mux::socket_capacity(requested);
    if max_connections < 32 {
        return Err("Not enough file descriptors for the tunnel".into());
    }
    let route_description = options.jumps.iter().map(|jump| jump.server.name.as_str())
        .chain(std::iter::once(server.name.as_str())).collect::<Vec<_>>().join(" → ");
    if !options.jumps.is_empty() {
        super::log_line(&logs, format!("SSH-маршрут: {route_description}"));
    }
    let route_note = options.route_note.clone();
    let route = tokio::time::timeout(route_connect_timeout(&options), connect_route(&server, options))
    .await
    .map_err(|_| format!("Embedded SSH connect/authentication timed out: {route_description}"))??;
    if let Some(note) = route_note.and_then(|note| note.lock().ok().and_then(|mut slot| slot.take())) {
        super::log_line(&logs, note);
    }
    super::log_line(&logs, "SSH и ключ проверены; проверяем серверный агент");
    let tcp_interactive_bytes = ssh_tuning.tcp_interactive_bytes;
    let (transport, agent) = start_agent_transport_qos(
        &route.destination,
        max_connections,
        udp_receive_window,
        agent_udp_receive_window,
        cap_default_udp_receive_slots,
        tcp_interactive_bytes,
        ssh_tuning.tcp_receive_window_bytes,
    )
    .await?;
    super::log_line(&logs, agent.summary());
    super::log_line(
        &logs,
        format!(
            "Transport v3 ready: one SSH channel, {} flows, bounded credit windows",
            transport.snapshot().limit
        ),
    );
    let ipv6_supported = if probe_ipv6 {
        async fn reachable(transport: &Endpoint, host: &str) -> bool {
            let Ok(target) = udp_protocol::encode_target(host, 443) else {
                return false;
            };
            matches!(
                tokio::time::timeout(
                    Duration::from_secs(3),
                transport.probe_tcp(target)
                )
                .await,
                Ok(Ok(_))
            )
        }
        let (cloudflare, google) = tokio::join!(
            reachable(&transport, "2606:4700:4700::1111"),
            reachable(&transport, "2001:4860:4860::8888")
        );
        let supported = cloudflare || google;
        super::log_line(
            &logs,
            if supported {
                "Android: server IPv6 egress available"
            } else {
                "Android: server IPv6 egress unavailable; using IPv4 compatibility mode"
            },
        );
        Some(supported)
    } else {
        None
    };
    let buffers = DatagramBuffers::default();
    let connection_limit = Arc::new(tokio::sync::Semaphore::new(transport.snapshot().limit + 32));
    let capacity = Arc::new(CapacityRejections::default());
    let _ = ready.send((ipv6_supported, transport.clone(), agent, route.outer_tcp.clone(), ssh_tuning));
    let mut tcp_tick = tokio::time::interval(Duration::from_secs(5));
    tcp_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if *stop.borrow() {
            transport.shutdown();
            route.disconnect().await;
            return Ok(());
        }
        tokio::select! {
            _ = tcp_tick.tick(), if log_tcp => {
                if let Ok(snapshot) = serde_json::to_string(&route.outer_tcp.snapshot()) {
                    super::log_line(&logs, format!("SSH TCP {snapshot}"));
                }
                if let Ok(snapshot) = serde_json::to_string(&transport.snapshot().diagnostics.heartbeat) {
                    super::log_line(&logs, format!("SSH heartbeat {snapshot}"));
                }
            }
            accepted = listener.accept() => {
                let (stream, originator_addr) = accepted.map_err(|error| error.to_string())?;
                let permit = match Arc::clone(&connection_limit).try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => { super::log_line(&logs, "SOCKS admission capacity reached"); continue; }
                };
                let transport = transport.clone();
                let counters = counters.clone(); let logs = logs.clone(); let buffers = buffers.clone();
                let capacity = capacity.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(error) = handle_socks_client(stream, originator_addr, transport, counters, &logs, buffers).await {
                        if CapacityRejections::is_capacity(&error) {
                            capacity.note(&logs, &error);
                        } else if !secret_tunnel_mux::is_flow_cancellation(&error) {
                            super::log_line(&logs, format!("SOCKS connection: {error}"));
                        }
                    }
                });
            }
            _ = stop.changed() => { transport.shutdown(); route.disconnect().await; return Ok(()); }
            _ = transport.closed() => return Err(transport.error().unwrap_or_else(|| "Transport stopped".into())),
        }
    }
}

/// The transport refuses new flows in bursts of hundreds once a session budget
/// is exhausted. Name the cause once per interval instead of filling the bounded
/// log with identical lines; the tunnel itself keeps running.
#[derive(Default)]
struct CapacityRejections {
    total: AtomicU64,
    reported_at_ms: AtomicU64,
}

impl CapacityRejections {
    const REPORT_INTERVAL_MS: u64 = 10_000;

    /// Reasons produced by either multiplexer endpoint when it cannot admit a flow.
    fn is_capacity(error: &str) -> bool {
        matches!(
            error,
            "Receive buffer budget reached"
                | "Receive packet budget reached"
                | "Connection capacity reached"
                | "Accept queue capacity reached"
        )
    }

    fn note(&self, logs: &Arc<std::sync::Mutex<Vec<String>>>, error: &str) {
        let total = self.total.fetch_add(1, Ordering::Relaxed) + 1;
        let now = secret_tunnel_mux::unix_millis();
        let reported = self.reported_at_ms.load(Ordering::Relaxed);
        if now.saturating_sub(reported) >= Self::REPORT_INTERVAL_MS
            && self
                .reported_at_ms
                .compare_exchange(reported, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            super::log_line(
                logs,
                format!(
                    "Туннель отклоняет новые соединения: достигнут предел потоков сеанса ({error}). Отказов с начала сеанса: {total}"
                ),
            );
        }
    }
}

async fn discover_agent(session: &client::Handle<SshClient>) -> Result<AgentCapabilities, String> {
    let mut channel = tokio::time::timeout(Duration::from_secs(4), session.channel_open_session())
        .await
        .map_err(|_| {
            "AGENT_CHECK_TIMEOUT: SSH и ключ проверены; канал диагностики не ответил".to_string()
        })?
        .map_err(|e| format!("SSH agent channel: {e}"))?;
    let result = tokio::time::timeout(Duration::from_secs(4), async {
        channel
            .exec(true, agent::QUERY_COMMAND)
            .await
            .map_err(|e| e.to_string())?;
        // Noninteractive even with pre-metadata agents; no stdin is required.
        channel.eof().await.map_err(|e| e.to_string())?;
        let (mut stdout, mut stderr, mut exit) = (Vec::new(), Vec::new(), None);
        while let Some(message) = channel.wait().await {
            match message {
                russh::ChannelMsg::Data { data } => {
                    if stdout.len() + stderr.len() + data.len() > agent::OUTPUT_LIMIT {
                        return Err(agent::invalid_response());
                    }
                    stdout.extend_from_slice(&data);
                }
                russh::ChannelMsg::ExtendedData { data, .. } => {
                    if stdout.len() + stderr.len() + data.len() > agent::OUTPUT_LIMIT {
                        return Err(agent::invalid_response());
                    }
                    stderr.extend_from_slice(&data);
                }
                russh::ChannelMsg::ExitStatus { exit_status } => exit = Some(exit_status),
                russh::ChannelMsg::Close => break,
                russh::ChannelMsg::Failure => {
                    return Err(
                        "AGENT_CHECK_FAILED: Сервер запретил запуск диагностики агента".into(),
                    )
                }
                _ => {}
            }
        }
        agent::parse(exit, &stdout, &stderr)
    })
    .await
    .map_err(|_| {
        "AGENT_CHECK_TIMEOUT: SSH и ключ проверены; агент не ответил за 4 секунды".to_string()
    });
    let _ = channel.close().await;
    result?
}

#[cfg(test)]
async fn start_agent_transport(
    session: &client::Handle<SshClient>,
    max_connections: usize,
    udp_receive_window: Option<FlowWindow>,
    agent_udp_receive_window: Option<FlowWindow>,
    cap_default_udp_receive_slots: bool,
) -> Result<(Endpoint, AgentCapabilities), String> {
    start_agent_transport_qos(
        session,
        max_connections,
        udp_receive_window,
        agent_udp_receive_window,
        cap_default_udp_receive_slots,
        None,
        secret_tunnel_mux::TCP_RECEIVE_WINDOW,
    )
    .await
}

async fn start_agent_transport_qos(
    session: &client::Handle<SshClient>,
    max_connections: usize,
    udp_receive_window: Option<FlowWindow>,
    agent_udp_receive_window: Option<FlowWindow>,
    cap_default_udp_receive_slots: bool,
    tcp_interactive_bytes: Option<usize>,
    tcp_receive_window: usize,
) -> Result<(Endpoint, AgentCapabilities), String> {
    let discovery = discover_agent(session).await?;
    if tcp_interactive_bytes.is_some() && !discovery.qos {
        return Err("AGENT_INCOMPATIBLE: Для приоритета UDP обновите серверный агент.".into());
    }
    let udp_receive_window = udp_receive_window.map(|mut window| {
        if cap_default_udp_receive_slots {
            window.slots = window.slots.min(discovery.max_udp_slots);
        }
        window
    });
    if agent_udp_receive_window.is_some() && (!discovery.flow_control || !discovery.udp_config) {
        return Err("AGENT_INCOMPATIBLE: Для настройки окна отправки UDP обновите серверный агент или выберите окно сервера по умолчанию.".into());
    }
    if [udp_receive_window, agent_udp_receive_window]
        .into_iter()
        .flatten()
        .any(|window| !discovery.flow_control || window.slots > discovery.max_udp_slots)
    {
        return Err(format!(
            "AGENT_INCOMPATIBLE: Серверный агент не поддерживает выбранное окно UDP. Лимит агента — {} слотов.",
            discovery.max_udp_slots
        ));
    }
    let transport = tokio::time::timeout(Duration::from_secs(10), async {
        let channel = session.channel_open_session().await.map_err(|e| e.to_string())?;
        let command = if tcp_interactive_bytes.is_some() {
            agent::QOS_COMMAND
        } else if agent_udp_receive_window.is_some() {
            agent::UDP_CONFIG_COMMAND
        } else if discovery.flow_control {
            agent::FLOW_CONTROL_COMMAND
        } else if discovery.diagnostics {
            agent::DIAGNOSTICS_COMMAND
        } else {
            agent::START_COMMAND
        };
        // Both ends must grow TCP sending gradually before either announces a
        // window above the baseline; an older agent keeps the 64 KiB behaviour.
        let elastic_tcp = discovery.elastic_tcp;
        channel
            .exec(true, agent::command(command, elastic_tcp))
            .await
            .map_err(|e| e.to_string())?;
        let mut stream = channel.into_stream();
        if let Some(bytes) = tcp_interactive_bytes {
            agent::write_session_settings(&mut stream,agent::SessionSettings {
                udp_window:agent_udp_receive_window.unwrap_or(secret_tunnel_mux::UDP_WINDOW), tcp_interactive_bytes:bytes,
            }).await.map_err(|error|error.to_string())?;
        } else if let Some(window) = agent_udp_receive_window {
            agent::write_udp_window_request(&mut stream, window)
                .await
                .map_err(|error| error.to_string())?;
        }
        let transport = Endpoint::start(stream, Role::Client, MuxConfig {
            max_flows: max_connections,
            // Our receive window bounds the download rate of each connection.
            receive_window: if elastic_tcp {
                tcp_receive_window
            } else {
                secret_tunnel_mux::BASE_TCP_WINDOW
            },
            heartbeat_initiator: discovery.heartbeat,
            tcp_interactive_bytes,
            udp_window: discovery
                .flow_control
                .then_some(udp_receive_window.unwrap_or(secret_tunnel_mux::UDP_WINDOW)),
            ..Default::default()
        }).await?;
        if agent_udp_receive_window.is_some_and(|window| transport.snapshot().peer_udp_window != Some(window)) {
            return Err("Серверный агент вернул другое окно отправки UDP".into());
        }
        Ok::<_, String>(transport)
    }).await.map_err(|_| {
        "AGENT_START_TIMEOUT: SSH и ключ проверены; агент не начал STMUX003 за 10 секунд".to_string()
    })?.map_err(|error| {
        let incompatible = error.contains("STMUX003") || error.contains("Invalid peer resource");
        if incompatible {
            format!("AGENT_INCOMPATIBLE: SSH и ключ проверены. Требуется агент STMUX003 с TCP/UDP; обновите ~/.local/bin/secret-tunnel-agent. {error}")
        } else {
            format!("AGENT_START_FAILED: SSH и ключ проверены; соединение с агентом прервалось. {error}")
        }
    })?;
    Ok((transport, discovery))
}

/// Authenticate and verify the data handshake without binding SOCKS or routes.
pub fn diagnose_server(server: EmbeddedServer, pin: Option<String>) -> Result<String, String> {
    diagnose_server_with_options(
        server,
        EmbeddedOptions { host_key_sha256: pin, ..Default::default() },
        true,
    )
}

pub fn diagnose_server_with_options(
    server: EmbeddedServer,
    options: EmbeddedOptions,
    check_agent: bool,
) -> Result<String, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let timeout = route_connect_timeout(&options) + Duration::from_secs(15);
    let tcp_interactive_bytes = options.ssh_tuning.tcp_interactive_bytes;
    let tcp_receive_window = options.ssh_tuning.tcp_receive_window_bytes;
    runtime.block_on(async {
        tokio::time::timeout(timeout, async {
            let route = connect_route(&server, options).await?;
            let result = if check_agent {
                start_agent_transport_qos(
                    &route.destination,
                    64,
                    None,
                    None,
                    false,
                    tcp_interactive_bytes,
                    tcp_receive_window,
                )
                .await
                .map(|(transport, discovery)| {
                    transport.shutdown();
                    format!(
                        "SSH и ключ: OK. {}. Проверка STMUX003: OK.",
                        discovery.summary()
                    )
                })
            } else {
                Ok("SSH и ключ: OK.".into())
            };
            route.disconnect().await;
            result
        })
        .await
        .map_err(|_| format!("Проверка SSH/агента не завершилась за {} с", timeout.as_secs()))?
    })
}

/// The primary physical interface macOS reports (`scutil --nwi`), with its
/// IPv4 address. Other VPNs' TUN interfaces are not listed there.
#[cfg(target_os = "macos")]
fn primary_interface() -> Option<(String, Option<std::net::IpAddr>)> {
    let output = std::process::Command::new("/usr/sbin/scutil").arg("--nwi").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut name = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if name.is_none() {
            if let Some((interface, rest)) = trimmed.split_once(" : flags") {
                if !interface.is_empty() && !interface.contains(' ') && rest.contains("IPv4") {
                    name = Some(interface.to_string());
                }
            }
        } else if let Some(address) = trimmed.strip_prefix("address") {
            let address = address.trim_start_matches([' ', ':']).trim().parse().ok();
            return name.map(|name| (name, address));
        } else if trimmed.contains(" : flags") {
            break;
        }
    }
    name.map(|name| (name, None))
}

#[cfg(target_os = "macos")]
fn bind_to_interface(socket: &tokio::net::TcpSocket, name: &str, ipv6: bool) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    let c_name = std::ffi::CString::new(name).map_err(|e| e.to_string())?;
    let index = unsafe { libc::if_nametoindex(c_name.as_ptr()) };
    if index == 0 {
        return Err(format!("интерфейс {name} не найден"));
    }
    let (level, option) = if ipv6 { (libc::IPPROTO_IPV6, libc::IPV6_BOUND_IF) } else { (libc::IPPROTO_IP, libc::IP_BOUND_IF) };
    let index = index as libc::c_int;
    let result = unsafe {
        libc::setsockopt(socket.as_raw_fd(), level, option, &index as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t)
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

async fn handshake<A: ToSocketAddrs>(
    server: &EmbeddedServer,
    addrs: A,
    options: EmbeddedOptions,
    observer: Arc<TcpObserver>,
) -> Result<client::Handle<SshClient>, String> {
    // Create and protect the transport before connect: VPN routes must never
    // capture their own SSH connection. No extra transport connection is made.
    let mut connected = None;
    let mut last_error = "SSH hostname has no addresses".to_string();
    #[cfg(target_os = "macos")]
    let primary = (options.protect_socket.is_none()).then(primary_interface).flatten();
    #[cfg(target_os = "macos")]
    let mut bound_to: Option<String> = None;
    for addr in tokio::net::lookup_host(addrs)
        .await
        .map_err(|e| e.to_string())?
    {
        let socket = if addr.is_ipv4() {
            tokio::net::TcpSocket::new_v4()
        } else {
            tokio::net::TcpSocket::new_v6()
        }
        .map_err(|e| e.to_string())?;
        #[cfg(target_os = "macos")]
        if options.bind_physical_interface {
            if let Some((name, _)) = &primary {
                match bind_to_interface(&socket, name, addr.is_ipv6()) {
                    Ok(()) => bound_to = Some(name.clone()),
                    Err(error) => last_error = format!("Не удалось привязать SSH к {name}: {error}"),
                }
            }
        }
        if let Some(protect) = &options.protect_socket {
            #[cfg(unix)]
            {
                use std::os::fd::AsRawFd;
                if !protect(socket.as_raw_fd()) {
                    return Err("Android refused to protect SSH socket".into());
                }
            }
            #[cfg(not(unix))]
            {
                let _ = protect;
                return Err("Socket protection is only supported on Unix".into());
            }
        }
        match socket.connect(addr).await {
            Ok(stream) => {
                connected = Some(stream);
                break;
            }
            Err(e) => last_error = e.to_string(),
        }
    }
    let stream = connected.ok_or(last_error)?;
    let _ = stream.set_nodelay(true);
    #[cfg(target_os = "macos")]
    if let Some(note) = &options.route_note {
        let local = stream.local_addr().ok().map(|addr| addr.ip());
        let text = match (&bound_to, &primary) {
            (Some(name), _) => Some(format!("SSH идёт напрямую через {name}, мимо других VPN")),
            (None, Some((name, Some(address)))) if local.is_some_and(|local| local != *address) => Some(format!(
                "SSH к серверу идёт через другой VPN или TUN (локальный адрес {}, а у {name} — {address}). Скорость может быть ниже: включите «SSH мимо других VPN» в настройках или исключите сервер в правилах того VPN.",
                local.unwrap()
            )),
            _ => None,
        };
        if let Ok(mut slot) = note.lock() {
            *slot = text;
        }
    }
    handshake_stream(server, ObservedStream::new(stream, observer), options).await
}

async fn handshake_stream<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    server: &EmbeddedServer,
    stream: S,
    options: EmbeddedOptions,
) -> Result<client::Handle<SshClient>, String> {
    options.ssh_tuning.validate()?;
    let mut config = client::Config {
        nodelay: true,
        channel_buffer_size: 32,
        inactivity_timeout: Some(Duration::from_secs(30)),
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        ..Default::default()
    };
    options.ssh_tuning.configure(&mut config);
    let config = Arc::new(config);
    let handler = SshClient {
        host: server.host.clone(),
        port: server.port,
        strict_host_key_checking: server.strict_host_key_checking.clone(),
        options,
    };
    client::connect_stream(config, stream, handler)
        .await
        .map_err(|error| friendly_connect_error(server, error))
}

/// Observe a host key without loading credentials or authenticating. The handler
/// rejects the key: discovery must never implicitly establish trust.
pub fn probe_server(host: &str, port: u16, jumps: Vec<EmbeddedJump>) -> Result<String, String> {
    let observed = Arc::new(std::sync::Mutex::new(None));
    let server = EmbeddedServer {
        name: "Android".into(),
        host: host.into(),
        port,
        user: String::new(),
        key_path: String::new(),
        strict_host_key_checking: "yes".into(),
    };
    let options = EmbeddedOptions {
        host_key_sha256: Some(String::new()),
        observed_host_key: Some(observed.clone()),
        jumps,
        ..Default::default()
    };
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let result = runtime.block_on(async {
        tokio::time::timeout(route_connect_timeout(&options), route_handshake(&server, options)).await
    });
    let seen = observed.lock().map_err(|e| e.to_string())?.clone();
    if let Some(pin) = seen {
        return Ok(pin);
    }
    Err(match result {
        Ok(Err(e)) => e,
        _ => "SSH server did not present a key before the connection timeout".into(),
    })
}

pub fn verify_login(server: EmbeddedServer, pin: String, jumps: Vec<EmbeddedJump>) -> Result<(), String> {
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let options = EmbeddedOptions { host_key_sha256: Some(pin), jumps, ..Default::default() };
    runtime.block_on(async {
        let route = tokio::time::timeout(route_connect_timeout(&options), connect_route(&server, options))
        .await
        .map_err(|_| "SSH authentication timed out".to_string())??;
        route.disconnect().await;
        Ok(())
    })
}

/// Register one device key after authenticating with the existing account key.
/// Preserve all pre-existing authorized_keys entries; never touch server services.
pub fn register_device_key(
    server: EmbeddedServer,
    pin: String,
    public_key: String,
    jumps: Vec<EmbeddedJump>,
) -> Result<(), String> {
    if !public_key.starts_with("ssh-ed25519 ")
        || public_key.len() > 512
        || !public_key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " +/=@._-".contains(c))
    {
        return Err("Invalid device public key".into());
    }
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let options = EmbeddedOptions { host_key_sha256: Some(pin), jumps, ..Default::default() };
    let timeout = route_connect_timeout(&options) + Duration::from_secs(5);
    runtime.block_on(async {
        tokio::time::timeout(timeout, async {
            let route = connect_route(&server, options).await?;
            let mut channel = route.destination.channel_open_session().await.map_err(|e| e.to_string())?;
            let command = format!("set -eu; umask 077; mkdir -p \"$HOME/.ssh\"; touch \"$HOME/.ssh/authorized_keys\"; grep -qxF '{}' \"$HOME/.ssh/authorized_keys\" || printf '\\n%s\\n' '{}' >> \"$HOME/.ssh/authorized_keys\"", public_key, public_key);
            channel.exec(true, command).await.map_err(|e| e.to_string())?;
            let mut success = false;
            while let Some(message) = channel.wait().await {
                if let russh::ChannelMsg::ExitStatus { exit_status } = message { success = exit_status == 0; }
            }
            route.disconnect().await;
            if success { Ok(()) } else { Err("Could not add device key to authorized_keys".into()) }
        }).await.map_err(|_| "Device registration timed out".to_string())?
    })
}

#[cfg(test)]
async fn connect_ssh<A: ToSocketAddrs>(
    server: &EmbeddedServer,
    addrs: A,
    options: EmbeddedOptions,
) -> Result<client::Handle<SshClient>, String> {
    let key_pair = load_identity(server)?;
    let mut session = handshake(server, addrs, options, Arc::new(TcpObserver::default())).await?;
    authenticate_ssh(server, &mut session, key_pair).await?;
    Ok(session)
}

fn load_identity(server: &EmbeddedServer) -> Result<ssh_key::PrivateKey, String> {
    load_secret_key(&server.key_path, None)
        .map_err(|error| friendly_key_error(&server.key_path, error))
}

async fn authenticate_ssh(
    server: &EmbeddedServer,
    session: &mut client::Handle<SshClient>,
    key_pair: ssh_key::PrivateKey,
) -> Result<(), String> {
    let hash = session
        .best_supported_rsa_hash()
        .await
        .map_err(|error| error.to_string())?
        .flatten();
    let auth = session
        .authenticate_publickey(
            server.user.clone(),
            PrivateKeyWithHashAlg::new(Arc::new(key_pair), hash),
        )
        .await
        .map_err(|error| friendly_auth_error(server, error))?;

    if !auth.success() {
        return Err(format!(
            "Embedded SSH public key authentication failed for {}@{}. Check username, key path, authorized_keys, and key permissions.",
            server.user, server.host
        ));
    }

    Ok(())
}

fn route_connect_timeout(options: &EmbeddedOptions) -> Duration {
    Duration::from_secs(if options.jumps.is_empty() { 15 } else { 25 })
}

async fn handshake_via(
    server: &EmbeddedServer,
    options: EmbeddedOptions,
    previous: Option<&client::Handle<SshClient>>,
    observer: Arc<TcpObserver>,
) -> Result<client::Handle<SshClient>, String> {
    if let Some(previous) = previous {
        let channel = previous.channel_open_direct_tcpip(&server.host, server.port.into(), "127.0.0.1", 0)
            .await
            .map_err(|error| format!("SSH-проброс до {}:{} отклонён или недоступен: {error}", server.host, server.port))?;
        handshake_stream(server, channel.into_stream(), options).await
    } else {
        handshake(server, (server.host.as_str(), server.port), options, observer).await
    }
}

async fn route_handshake(server: &EmbeddedServer, mut options: EmbeddedOptions) -> Result<SshRoute, String> {
    options.ssh_tuning.validate()?;
    let outer_tcp = Arc::new(TcpObserver::default());
    let jumps = std::mem::take(&mut options.jumps);
    if jumps.len() > MAX_SSH_JUMPS {
        return Err(format!("Допустимо не более {MAX_SSH_JUMPS} промежуточных SSH-серверов"));
    }
    let mut endpoints = std::collections::HashSet::from([(server.host.to_ascii_lowercase(), server.port)]);
    for jump in &jumps {
        if !endpoints.insert((jump.server.host.to_ascii_lowercase(), jump.server.port)) {
            return Err("SSH-маршрут повторяет адрес сервера".into());
        }
    }
    let mut sessions = Vec::with_capacity(jumps.len());
    for (index, jump) in jumps.into_iter().enumerate() {
        let hop_options = EmbeddedOptions {
            ssh_tuning: options.ssh_tuning.clone(),
            host_key_sha256: jump.host_key_sha256,
            protect_socket: options.protect_socket.clone(),
            bind_physical_interface: options.bind_physical_interface,
            route_note: options.route_note.clone(),
            ..Default::default()
        };
        let result = async {
            let identity = load_identity(&jump.server)?;
            let mut session = handshake_via(&jump.server, hop_options, sessions.last(), outer_tcp.clone()).await?;
            authenticate_ssh(&jump.server, &mut session, identity).await?;
            Ok::<_, String>(session)
        }.await;
        sessions.push(result.map_err(|error| format!("SSH-звено {} ({}): {error}", index + 1, jump.server.name))?);
    }
    let destination = handshake_via(server, options, sessions.last(), outer_tcp.clone()).await?;
    Ok(SshRoute { destination, jumps: sessions, outer_tcp })
}

async fn connect_route(server: &EmbeddedServer, options: EmbeddedOptions) -> Result<SshRoute, String> {
    let identity = load_identity(server)?;
    let mut route = route_handshake(server, options).await?;
    authenticate_ssh(server, &mut route.destination, identity).await?;
    Ok(route)
}

async fn handle_socks_client(
    mut stream: TcpStream,
    originator_addr: std::net::SocketAddr,
    transport: Endpoint,
    counters: Arc<TrafficCounters>,
    logs: &Arc<std::sync::Mutex<Vec<String>>>,
    buffers: DatagramBuffers,
) -> Result<(), String> {
    let mut request =
        match tokio::time::timeout(Duration::from_secs(10), read_socks5_request(&mut stream))
            .await
            .map_err(|_| "SOCKS handshake timed out".to_string())?
        {
            Ok(request) => request,
            Err(error) => {
                let _ = stream.shutdown().await;
                return Err(error);
            }
        };

    if request.command == 0x03 {
        return relay_udp_association(
            stream,
            originator_addr,
            request.port,
            transport,
            counters,
            buffers,
            DNS_IDLE,
        )
        .await;
    }

    // DNS over TCP goes to the same reachable resolver as DNS over UDP.
    if request.port == TUNNEL_DNS.port() && request.host.parse::<std::net::IpAddr>().is_ok() {
        request.host = TUNNEL_DNS.ip().to_string();
    }
    let fake_ip = tls_target::is_fake_ip(&request.host);
    let (destination, preface) = if fake_ip {
        // The proxy client sends ClientHello only after SOCKS CONNECT succeeds.
        // Delay opening the SSH channel until its visible server name is known.
        write_socks5_reply(&mut stream, 0x00).await?;
        let recovered = tokio::time::timeout(
            Duration::from_secs(10),
            tls_target::read_server_name(&mut stream),
        )
        .await
        .map_err(|_| "Timed out recovering TLS server name for local fake-IP destination")??;
        super::log_line(
            logs,
            "SOCKS: local fake-IP destination restored from TLS server name",
        );
        recovered
    } else {
        (request.host.clone(), Vec::new())
    };
    if std::env::var_os("SECRET_TUNNEL_TRACE_DESTINATIONS").is_some() {
        super::log_line(
            logs,
            format!(
                "SOCKS destination: {}:{} -> {}:{}",
                request.host, request.port, destination, request.port
            ),
        );
    }

    let target =
        udp_protocol::encode_target(&destination, request.port).map_err(|e| e.to_string())?;
    let mut flow = match transport.open(FlowKind::Tcp, target).await {
        Ok(flow) => flow,
        Err(error) => {
            if !fake_ip {
                let _ = write_socks5_reply(&mut stream, 0x04).await;
            }
            return Err(error);
        }
    };

    if !fake_ip {
        write_socks5_reply(&mut stream, 0x00).await?;
    }
    let usage = app_usage::FlowUsage::start(false, &format!("{}:{}", request.host, request.port));
    if !preface.is_empty() {
        for chunk in preface.chunks(secret_tunnel_mux::TCP_CHUNK) {
            flow.sender().send(chunk.to_vec()).await?;
        }
        counters.add_tx(preface.len() as u64);
        usage.add_tx(preface.len() as u64);
    }
    let relay_started = std::time::Instant::now();
    let result = secret_tunnel_mux::relay(&mut flow, &mut stream, |direction, count| match direction {
        secret_tunnel_mux::Direction::Upload => {
            counters.add_tx(count as u64);
            usage.add_tx(count as u64);
        }
        secret_tunnel_mux::Direction::Download => {
            counters.add_rx(count as u64);
            usage.add_rx(count as u64);
        }
    })
    .await;
    if let Err(error) = &result {
        flow.record_tcp(secret_tunnel_mux::TcpStage::Relay, relay_started.elapsed(), Err(error));
    }
    result
}

#[derive(Clone, Debug)]
struct SocksRequest {
    command: u8,
    host: String,
    port: u16,
}

async fn read_socks5_request(stream: &mut TcpStream) -> Result<SocksRequest, String> {
    let mut greeting = [0_u8; 2];
    stream
        .read_exact(&mut greeting)
        .await
        .map_err(|error| error.to_string())?;
    if greeting[0] != 0x05 {
        return Err("Unsupported SOCKS version".to_string());
    }

    let methods_len = greeting[1] as usize;
    let mut methods = vec![0_u8; methods_len];
    stream
        .read_exact(&mut methods)
        .await
        .map_err(|error| error.to_string())?;
    if !methods.contains(&0x00) {
        stream
            .write_all(&[0x05, 0xff])
            .await
            .map_err(|error| error.to_string())?;
        return Err("SOCKS client did not offer no-auth method".to_string());
    }
    stream
        .write_all(&[0x05, 0x00])
        .await
        .map_err(|error| error.to_string())?;

    let mut head = [0_u8; 4];
    stream
        .read_exact(&mut head)
        .await
        .map_err(|error| error.to_string())?;
    if head[0] != 0x05 {
        return Err("Unsupported SOCKS request version".to_string());
    }
    if head[2] != 0 || !matches!(head[1], 0x01 | 0x03) {
        write_socks5_reply(stream, 0x07).await?;
        return Err("Only SOCKS CONNECT and UDP ASSOCIATE are supported".to_string());
    }

    let host = match head[3] {
        0x01 => {
            let mut octets = [0_u8; 4];
            stream
                .read_exact(&mut octets)
                .await
                .map_err(|error| error.to_string())?;
            format!("{}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3])
        }
        0x03 => {
            let mut len = [0_u8; 1];
            stream
                .read_exact(&mut len)
                .await
                .map_err(|error| error.to_string())?;
            let mut name = vec![0_u8; len[0] as usize];
            stream
                .read_exact(&mut name)
                .await
                .map_err(|error| error.to_string())?;
            String::from_utf8(name).map_err(|error| error.to_string())?
        }
        0x04 => {
            let mut octets = [0_u8; 16];
            stream
                .read_exact(&mut octets)
                .await
                .map_err(|error| error.to_string())?;
            std::net::Ipv6Addr::from(octets).to_string()
        }
        _ => {
            write_socks5_reply(stream, 0x08).await?;
            return Err("Unsupported SOCKS address type".to_string());
        }
    };

    let mut port = [0_u8; 2];
    stream
        .read_exact(&mut port)
        .await
        .map_err(|error| error.to_string())?;

    Ok(SocksRequest {
        command: head[1],
        host,
        port: u16::from_be_bytes(port),
    })
}

/// Idle limits for an association whose every datagram went to port 53.
///
/// A DNS exchange ends within milliseconds, yet a TUN adapter keeps the SOCKS
/// association for its own idle timeout (60 s in hev-socks5-tunnel) and each
/// resolver query opens a new one. Every association holds a flow and a whole
/// receive window on both ends, so a burst of lookups could exhaust the session.
/// Closing a quiet DNS association early returns those; the adapter transparently
/// opens a new association for a later datagram from the same socket.
#[derive(Clone, Copy)]
struct DnsIdle {
    /// After at least one reply was relayed back.
    answered: Duration,
    /// While no reply has arrived: covers resolver retransmissions.
    unanswered: Duration,
}

const DNS_IDLE: DnsIdle = DnsIdle {
    answered: Duration::from_secs(5),
    unanswered: Duration::from_secs(15),
};

/// Resolver for DNS that intercepted applications send through the tunnel.
/// The system resolver is often reachable only from this computer: a home
/// router, or another VPN's TUN that hijacks port 53 (Clash answers its
/// 114.114.114.114 locally). Sent from the server, such queries time out and
/// the application cannot resolve anything.
const TUNNEL_DNS: std::net::SocketAddr =
    std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(1, 1, 1, 1)), 53);

/// A DNS datagram re-addressed to [`TUNNEL_DNS`], with the server it was for.
fn redirect_dns(packet: &[u8]) -> Option<(Vec<u8>, std::net::SocketAddr)> {
    let parsed = udp_protocol::parse_packet(packet).ok()?;
    let udp_protocol::PacketAddress::Socket(address) = parsed.address else { return None };
    if address.port() != 53 || address == TUNNEL_DNS {
        return None;
    }
    Some((udp_protocol::encode_packet(TUNNEL_DNS, parsed.payload).ok()?, address))
}

/// A reply from [`TUNNEL_DNS`] shown as coming from the server the
/// application asked, since resolvers drop answers from other addresses.
fn restore_dns_source(packet: &[u8], original: std::net::SocketAddr) -> Option<Vec<u8>> {
    let parsed = udp_protocol::parse_packet(packet).ok()?;
    if parsed.address != udp_protocol::PacketAddress::Socket(TUNNEL_DNS) {
        return None;
    }
    udp_protocol::encode_packet(original, parsed.payload).ok()
}

async fn relay_udp_association(
    mut control: TcpStream,
    peer: std::net::SocketAddr,
    requested_port: u16,
    transport: Endpoint,
    counters: Arc<TrafficCounters>,
    buffers: DatagramBuffers,
    dns_idle: DnsIdle,
) -> Result<(), String> {
    let mut flow = match transport.open(FlowKind::Udp, Vec::new()).await {
        Ok(flow) => flow,
        Err(error) => {
            let _ = write_socks5_reply(&mut control, 0x01).await;
            return Err(error);
        }
    };
    let sender = flow.sender();
    let local = control.local_addr().map_err(|e| e.to_string())?;
    let socket = tokio::net::UdpSocket::bind((local.ip(), 0))
        .await
        .map_err(|e| e.to_string())?;
    let bound = socket.local_addr().map_err(|e| e.to_string())?;
    let mut reply = udp_protocol::encode_packet(bound, &[]).map_err(|e| e.to_string())?;
    reply[0] = 5;
    control.write_all(&reply).await.map_err(|e| e.to_string())?;
    let mut control_buffer = [0; 1];
    let mut endpoint = if requested_port == 0 {
        None
    } else {
        Some(std::net::SocketAddr::new(peer.ip(), requested_port))
    };
    // The application this association belongs to, from its first datagram.
    let mut usage: Option<Arc<app_usage::FlowUsage>> = None;
    // True while every datagram of this association went to port 53.
    let mut dns_only = true;
    // The resolver the application asked, while its queries go to TUNNEL_DNS.
    let mut dns_server: Option<std::net::SocketAddr> = None;
    let mut sent = false;
    let mut answered = false;
    let mut last_activity = tokio::time::Instant::now();
    loop {
        let dns_deadline = last_activity + if answered { dns_idle.answered } else { dns_idle.unanswered };
        tokio::select! {
            _ = control.read(&mut control_buffer) => return Ok(()),
            // Returning closes the control connection, which ends the association (RFC 1928).
            _ = tokio::time::sleep_until(dns_deadline), if dns_only && sent => return Ok(()),
            received = buffers.recv(&socket) => {
                let (packet, source) = received.map_err(|e| e.to_string())?;
                if source.ip() != peer.ip() || endpoint.is_some_and(|expected| expected != source) { continue; }
                let (payload, port) = match udp_protocol::parse_packet(&packet) {
                    Ok(parsed) => {
                        if usage.is_none() {
                            let destination = match parsed.address {
                                udp_protocol::PacketAddress::Socket(address) => address.to_string(),
                                udp_protocol::PacketAddress::Domain(host, port) => format!("{host}:{port}"),
                            };
                            usage = Some(app_usage::FlowUsage::start(true, &destination));
                        }
                        (parsed.payload.len(), parsed.address.port())
                    }
                    Err(_) => continue,
                };
                endpoint = Some(source);
                dns_only &= port == 53;
                let packet = match redirect_dns(&packet) {
                    Some((redirected, original)) => {
                        dns_server = Some(original);
                        redirected
                    }
                    None => packet,
                };
                sent = true;
                last_activity = tokio::time::Instant::now();
                if sender.try_send_datagram(packet)? {
                    counters.add_tx(payload as u64);
                    if let Some(usage) = &usage { usage.add_tx(payload as u64); }
                }
            }
            received = flow.recv() => {
                let Some(packet) = received? else { return Ok(()); };
                let restored = dns_server.and_then(|original| restore_dns_source(&packet, original));
                let datagram: &[u8] = restored.as_deref().unwrap_or(&packet);
                if let Some(endpoint) = endpoint {
                    socket.send_to(datagram, endpoint).await.map_err(|e| e.to_string())?;
                    let parsed = udp_protocol::parse_packet(datagram).map_err(|e| e.to_string())?;
                    counters.add_rx(parsed.payload.len() as u64);
                    if let Some(usage) = &usage { usage.add_rx(parsed.payload.len() as u64); }
                }
                answered = true;
                last_activity = tokio::time::Instant::now();
                drop(packet);
            }
        }
    }
}

async fn write_socks5_reply(stream: &mut TcpStream, status: u8) -> Result<(), String> {
    stream
        .write_all(&[0x05, status, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod udp_association_tests {
    use super::*;
    use secret_tunnel_mux::UDP_WINDOW;
    use std::net::SocketAddr;
    use tokio::net::{TcpListener, UdpSocket};

    const FAST: DnsIdle = DnsIdle {
        answered: Duration::from_millis(200),
        unanswered: Duration::from_millis(600),
    };

    struct Association {
        transport: Endpoint,
        app_control: TcpStream,
        app_udp: UdpSocket,
        relay: SocketAddr,
        task: tokio::task::JoinHandle<Result<(), String>>,
    }

    /// A real multiplexer pair in memory whose server side echoes datagrams
    /// (or stays silent), and one SOCKS UDP association over loopback sockets.
    async fn association(dns_idle: DnsIdle, echo: bool) -> Association {
        let (a, b) = tokio::io::duplex(1 << 16);
        let config = MuxConfig {
            max_flows: 8,
            udp_window: Some(UDP_WINDOW),
            ..Default::default()
        };
        let (transport, server) = tokio::join!(
            Endpoint::start(a, Role::Client, config.clone()),
            Endpoint::start(b, Role::Server, config)
        );
        let (transport, server) = (transport.unwrap(), server.unwrap());
        tokio::spawn(async move {
            while let Some(incoming) = server.accept().await {
                tokio::spawn(async move {
                    let mut flow = incoming.flow;
                    flow.accept().unwrap();
                    let sender = flow.sender();
                    while let Ok(Some(packet)) = flow.recv().await {
                        if echo {
                            let _ = sender.try_send_datagram(packet.to_vec());
                        }
                    }
                });
            }
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut app_control = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
        let (proxy_control, peer) = listener.accept().await.unwrap();
        let task = tokio::spawn(relay_udp_association(
            proxy_control,
            peer,
            0,
            transport.clone(),
            Arc::new(TrafficCounters::default()),
            DatagramBuffers::default(),
            dns_idle,
        ));
        let mut reply = [0_u8; 10];
        app_control.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[..4], [5, 0, 0, 1]);
        let relay = SocketAddr::from(([127, 0, 0, 1], u16::from_be_bytes([reply[8], reply[9]])));
        let app_udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        Association { transport, app_control, app_udp, relay, task }
    }

    async fn exchange(association: &Association, port: u16) {
        let packet = udp_protocol::encode_packet(([1, 1, 1, 1], port).into(), b"query").unwrap();
        association.app_udp.send_to(&packet, association.relay).await.unwrap();
        let mut buffer = [0_u8; 128];
        let (count, _) = tokio::time::timeout(
            Duration::from_secs(2),
            association.app_udp.recv_from(&mut buffer),
        )
        .await
        .expect("echo through the multiplexer")
        .unwrap();
        assert_eq!(&buffer[..count], &packet[..]);
    }

    #[tokio::test]
    async fn an_answered_dns_association_closes_early_and_frees_its_flow() {
        let mut association = association(FAST, true).await;
        exchange(&association, 53).await;
        assert_eq!(association.transport.snapshot().active, 1);
        let result = tokio::time::timeout(Duration::from_secs(2), &mut association.task)
            .await
            .expect("DNS association must end after its idle limit");
        assert_eq!(result.unwrap(), Ok(()));
        // The adapter sees the association end as its control connection closing.
        let mut byte = [0_u8; 1];
        assert_eq!(association.app_control.read(&mut byte).await.unwrap(), 0);
        assert_eq!(association.transport.snapshot().active, 0);
    }

    #[tokio::test]
    async fn an_unanswered_dns_association_waits_for_the_longer_limit() {
        let mut association = association(FAST, false).await;
        let packet = udp_protocol::encode_packet(([1, 1, 1, 1], 53).into(), b"query").unwrap();
        association.app_udp.send_to(&packet, association.relay).await.unwrap();
        // No reply yet: a resolver may still retransmit on this socket.
        tokio::time::sleep(FAST.answered * 2).await;
        assert!(!association.task.is_finished(), "closed before the unanswered limit");
        let result = tokio::time::timeout(Duration::from_secs(2), &mut association.task)
            .await
            .expect("unanswered DNS association must still end");
        assert_eq!(result.unwrap(), Ok(()));
        assert_eq!(association.transport.snapshot().active, 0);
    }

    #[tokio::test]
    async fn dns_to_a_local_resolver_goes_to_the_tunnel_resolver_and_returns_from_it() {
        // The echo server answers from wherever the datagram was sent.
        let association = association(FAST, true).await;
        let asked = SocketAddr::from(([114, 114, 114, 114], 53));
        let packet = udp_protocol::encode_packet(asked, b"query").unwrap();
        let (redirected, original) = redirect_dns(&packet).unwrap();
        assert_eq!(original, asked);
        let parsed = udp_protocol::parse_packet(&redirected).unwrap();
        assert_eq!(parsed.address, udp_protocol::PacketAddress::Socket(TUNNEL_DNS));
        assert_eq!(parsed.payload, b"query");
        association.app_udp.send_to(&packet, association.relay).await.unwrap();
        let mut buffer = [0_u8; 128];
        let (count, _) = tokio::time::timeout(Duration::from_secs(2), association.app_udp.recv_from(&mut buffer))
            .await
            .expect("answer through the multiplexer")
            .unwrap();
        // The application sees the answer from the server it asked.
        assert_eq!(&buffer[..count], &packet[..]);
        // Other traffic is untouched.
        assert!(redirect_dns(&udp_protocol::encode_packet(([9, 9, 9, 9], 443).into(), b"x").unwrap()).is_none());
    }

    #[tokio::test]
    async fn other_udp_associations_follow_only_their_control_connection() {
        for ports in [vec![443], vec![53, 443]] {
            let mut association = association(FAST, true).await;
            for port in ports {
                exchange(&association, port).await;
            }
            tokio::time::sleep(FAST.unanswered * 2).await;
            assert!(!association.task.is_finished(), "non-DNS association closed on idle");
            assert_eq!(association.transport.snapshot().active, 1);
            drop(association.app_control);
            let result = tokio::time::timeout(Duration::from_secs(2), &mut association.task)
                .await
                .unwrap();
            assert_eq!(result.unwrap(), Ok(()));
        }
    }

    #[test]
    fn capacity_rejections_are_recognised_and_reported_once_per_interval() {
        for reason in [
            "Receive buffer budget reached",
            "Receive packet budget reached",
            "Connection capacity reached",
            "Accept queue capacity reached",
        ] {
            assert!(CapacityRejections::is_capacity(reason));
        }
        assert!(!CapacityRejections::is_capacity("TCP connect timeout"));
        let logs = Arc::new(std::sync::Mutex::new(Vec::new()));
        let rejections = CapacityRejections::default();
        for _ in 0..500 {
            rejections.note(&logs, "Receive buffer budget reached");
        }
        let logs = logs.lock().unwrap();
        assert_eq!(logs.len(), 1);
        assert!(logs[0].contains("Receive buffer budget reached"));
        assert_eq!(rejections.total.load(Ordering::Relaxed), 500);
    }
}
