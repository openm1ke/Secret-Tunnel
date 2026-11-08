use crate::udp_protocol;
use russh::client;
use russh::keys::known_hosts::{check_known_hosts_path, known_host_keys_path, learn_known_hosts};
use russh::keys::{load_secret_key, ssh_key, PrivateKeyWithHashAlg};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, ToSocketAddrs};

#[path = "tls_target.rs"]
mod tls_target;
#[path = "udp_mux.rs"]
mod udp_mux;

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

/// Platform integration without changing desktop defaults. Pinning is mandatory
/// in the Android adapter; an empty pin observes the host key but rejects it.
#[derive(Clone, Default)]
pub struct EmbeddedOptions {
    pub host_key_sha256: Option<String>,
    pub observed_host_key: Option<Arc<std::sync::Mutex<Option<String>>>>,
    pub protect_socket: Option<Arc<dyn Fn(i32) -> bool + Send + Sync>>,
    pub probe_ipv6: bool,
}

#[derive(Debug)]
pub struct EmbeddedTunnelHandle {
    stop_tx: Sender<()>,
    join: Option<JoinHandle<()>>,
    counters: Arc<TrafficCounters>,
    outcome: Receiver<Result<(), String>>,
    ipv6_supported: Option<bool>,
}

impl EmbeddedTunnelHandle {
    pub fn ipv6_supported(&self) -> Option<bool> { self.ipv6_supported }
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

    pub fn stop(mut self) {
        let _ = self.stop_tx.send(());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
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

type SharedSession = Arc<client::Handle<SshClient>>;

#[cfg(test)]
mod platform_tests {
    use super::*;
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
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (outcome_tx, outcome) = mpsc::channel();
    let join = thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = outcome_tx.send(Err(error.to_string()));
                return;
            }
        };

        let result = runtime.block_on(run_embedded_tunnel(
            listener,
            server,
            stop_rx,
            tunnel_counters,
            ready_tx,
            logs,
            options,
        ));
        let _ = outcome_tx.send(result);
    });

    let readiness = ready_rx.recv_timeout(Duration::from_secs(20));
    if readiness.is_err() {
        let _ = stop_tx.send(());
        let _ = join.join();
        return Err(outcome
            .try_recv()
            .ok()
            .and_then(Result::err)
            .unwrap_or_else(|| "Embedded SSH startup failed".into()));
    }

    Ok(EmbeddedTunnelHandle {
        stop_tx,
        join: Some(join),
        counters,
        outcome,
        ipv6_supported: readiness.unwrap(),
    })
}

async fn run_embedded_tunnel(
    listener: TcpListener,
    server: EmbeddedServer,
    stop_rx: mpsc::Receiver<()>,
    counters: Arc<TrafficCounters>,
    ready: Sender<Option<bool>>,
    logs: Arc<std::sync::Mutex<Vec<String>>>,
    options: EmbeddedOptions,
) -> Result<(), String> {
    let listener =
        tokio::net::TcpListener::from_std(listener).map_err(|error| error.to_string())?;
    let probe_ipv6 = options.probe_ipv6;
    let session = tokio::time::timeout(
        Duration::from_secs(15),
        connect_ssh(&server, (server.host.as_str(), server.port), options),
    )
    .await
    .map_err(|_| "Embedded SSH connect/authentication timed out".to_string())??;
    let session = Arc::new(session);
    let ipv6_supported = if probe_ipv6 {
        async fn reachable(session: &SharedSession, host: &str) -> bool {
            match tokio::time::timeout(Duration::from_secs(3),
                session.channel_open_direct_tcpip(host, 443, "127.0.0.1", 0)).await {
                Ok(Ok(channel)) => { let _ = channel.close().await; true }
                _ => false,
            }
        }
        let (cloudflare, google) = tokio::join!(
            reachable(&session, "2606:4700:4700::1111"),
            reachable(&session, "2001:4860:4860::8888"));
        let supported = cloudflare || google;
        super::log_line(&logs, if supported { "Android: server IPv6 egress available" }
            else { "Android: server IPv6 egress unavailable; using IPv4 compatibility mode" });
        Some(supported)
    } else { None };
    let udp = Arc::new(udp_mux::UdpService::default());
    let connection_limit = Arc::new(tokio::sync::Semaphore::new(256));
    let _ = ready.send(ipv6_supported);

    loop {
        if stop_rx.try_recv().is_ok() {
            disconnect_ssh(&session).await;
            return Ok(());
        }

        tokio::select! {
            accepted = listener.accept() => {
                let (stream, originator_addr) = accepted.map_err(|error| error.to_string())?;
                let permit = match Arc::clone(&connection_limit).try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => { super::log_line(&logs, "SOCKS connection limit reached"); continue; }
                };
                let session = Arc::clone(&session);
                let counters = Arc::clone(&counters);
                let logs = Arc::clone(&logs);
                let udp = Arc::clone(&udp);
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(error) = handle_socks_client(stream, originator_addr, session, counters, &logs, udp).await {
                        super::log_line(&logs, format!("SOCKS connection: {error}"));
                    }
                });
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                if session.is_closed() { return Err("SSH connection closed".into()); }
            }
        }
    }
}

async fn handshake<A: ToSocketAddrs>(
    server: &EmbeddedServer,
    addrs: A,
    options: EmbeddedOptions,
) -> Result<client::Handle<SshClient>, String> {
    let config = Arc::new(client::Config {
        nodelay: true,
        inactivity_timeout: Some(Duration::from_secs(30)),
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        ..Default::default()
    });
    let handler = SshClient {
        host: server.host.clone(),
        port: server.port,
        strict_host_key_checking: server.strict_host_key_checking.clone(),
        options: options.clone(),
    };
    // Create and protect the transport before connect: VPN routes must never
    // capture their own SSH connection. No extra transport connection is made.
    let mut connected = None;
    let mut last_error = "SSH hostname has no addresses".to_string();
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
    let session = client::connect_stream(config, stream, handler)
        .await
        .map_err(|error| friendly_connect_error(server, error))?;

    Ok(session)
}

/// Observe a host key without loading credentials or authenticating. The handler
/// rejects the key: discovery must never implicitly establish trust.
pub fn probe_server(host: &str, port: u16) -> Result<String, String> {
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
        ..Default::default()
    };
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let result = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(15), async {
            handshake(&server, (host, port), options).await
        })
        .await
    });
    let seen = observed.lock().map_err(|e| e.to_string())?.clone();
    if let Some(pin) = seen {
        return Ok(pin);
    }
    Err(match result {
        Ok(Err(e)) => e,
        _ => "SSH server did not present a key within 15 seconds".into(),
    })
}

pub fn verify_login(server: EmbeddedServer, pin: String) -> Result<(), String> {
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let session = tokio::time::timeout(
            Duration::from_secs(15),
            connect_ssh(
                &server,
                (server.host.as_str(), server.port),
                EmbeddedOptions {
                    host_key_sha256: Some(pin),
                    ..Default::default()
                },
            ),
        )
        .await
        .map_err(|_| "SSH authentication timed out".to_string())??;
        session
            .disconnect(russh::Disconnect::ByApplication, "", "English")
            .await
            .map_err(|e| e.to_string())
    })
}

/// Register one device key after authenticating with the existing account key.
/// Preserve all pre-existing authorized_keys entries; never touch server services.
pub fn register_device_key(
    server: EmbeddedServer,
    pin: String,
    public_key: String,
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
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let session = connect_ssh(&server, (server.host.as_str(), server.port),
                EmbeddedOptions { host_key_sha256: Some(pin), ..Default::default() }).await?;
            let mut channel = session.channel_open_session().await.map_err(|e| e.to_string())?;
            let command = format!("set -eu; umask 077; mkdir -p \"$HOME/.ssh\"; touch \"$HOME/.ssh/authorized_keys\"; grep -qxF '{}' \"$HOME/.ssh/authorized_keys\" || printf '\\n%s\\n' '{}' >> \"$HOME/.ssh/authorized_keys\"", public_key, public_key);
            channel.exec(true, command).await.map_err(|e| e.to_string())?;
            let mut success = false;
            while let Some(message) = channel.wait().await {
                if let russh::ChannelMsg::ExitStatus { exit_status } = message { success = exit_status == 0; }
            }
            let _ = session.disconnect(russh::Disconnect::ByApplication, "", "English").await;
            if success { Ok(()) } else { Err("Could not add device key to authorized_keys".into()) }
        }).await.map_err(|_| "Device registration timed out".to_string())?
    })
}

async fn connect_ssh<A: ToSocketAddrs>(
    server: &EmbeddedServer,
    addrs: A,
    options: EmbeddedOptions,
) -> Result<client::Handle<SshClient>, String> {
    let key_pair = load_secret_key(&server.key_path, None)
        .map_err(|error| friendly_key_error(&server.key_path, error))?;
    let mut session = handshake(server, addrs, options).await?;

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

    Ok(session)
}

async fn disconnect_ssh(session: &SharedSession) {
    let _ = session
        .disconnect(russh::Disconnect::ByApplication, "", "English")
        .await;
}

async fn handle_socks_client(
    mut stream: TcpStream,
    originator_addr: std::net::SocketAddr,
    session: SharedSession,
    counters: Arc<TrafficCounters>,
    logs: &Arc<std::sync::Mutex<Vec<String>>>,
    udp: Arc<udp_mux::UdpService>,
) -> Result<(), String> {
    let request =
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
        return relay_udp_association(stream, originator_addr, request.port, session, counters, &udp, logs)
            .await;
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

    let channel = tokio::time::timeout(Duration::from_secs(10), async {
        session
            .channel_open_direct_tcpip(
                destination,
                request.port.into(),
                originator_addr.ip().to_string(),
                originator_addr.port().into(),
            )
            .await
            .map_err(|error| error.to_string())
    }).await.map_err(|_| "SOCKS destination connect timed out".to_owned())
        .and_then(|result| result);
    let channel = match channel {
        Ok(channel) => channel,
        Err(error) => {
            if !fake_ip { let _ = write_socks5_reply(&mut stream, 0x04).await; }
            return Err(error);
        }
    };

    if !fake_ip {
        write_socks5_reply(&mut stream, 0x00).await?;
    }
    let mut channel_stream = channel.into_stream();
    if !preface.is_empty() {
        channel_stream
            .write_all(&preface)
            .await
            .map_err(|e| e.to_string())?;
        counters.add_tx(preface.len() as u64);
    }
    relay_with_counters(&mut stream, &mut channel_stream, &counters).await?;
    Ok(())
}

async fn relay_with_counters<A, B>(
    client: &mut A,
    remote: &mut B,
    counters: &TrafficCounters,
) -> Result<(), String>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let (mut client_read, mut client_write) = tokio::io::split(client);
    let (mut remote_read, mut remote_write) = tokio::io::split(remote);
    async fn copy_counted<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
        reader: &mut R,
        writer: &mut W,
        counter: &AtomicU64,
    ) -> Result<(), String> {
        let mut buffer = vec![0; 32768];
        loop {
            let count = reader.read(&mut buffer).await.map_err(|e| e.to_string())?;
            if count == 0 {
                return writer.shutdown().await.map_err(|e| e.to_string());
            }
            writer
                .write_all(&buffer[..count])
                .await
                .map_err(|e| e.to_string())?;
            counter.fetch_add(count as u64, Ordering::Relaxed);
        }
    }
    tokio::try_join!(
        copy_counted(&mut client_read, &mut remote_write, &counters.tx_total),
        copy_counted(&mut remote_read, &mut client_write, &counters.rx_total),
    )?;
    Ok(())
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

async fn relay_udp_association(
    mut control: TcpStream,
    peer: std::net::SocketAddr,
    requested_port: u16,
    session: SharedSession,
    counters: Arc<TrafficCounters>,
    udp: &udp_mux::UdpService,
    logs: &Arc<std::sync::Mutex<Vec<String>>>,
) -> Result<(), String> {
    let mut agent = udp.associate(&session, logs).await?;
    let local = control.local_addr().map_err(|e| e.to_string())?;
    let socket = tokio::net::UdpSocket::bind((local.ip(), 0))
        .await
        .map_err(|e| e.to_string())?;
    let bound = socket.local_addr().map_err(|e| e.to_string())?;
    let reply = udp_protocol::encode_packet(bound, &[]).map_err(|e| e.to_string())?;
    // UDP packet has an extra FRAG byte; SOCKS reply uses VER/REP/RSV.
    let mut reply = reply;
    reply[0] = 5;
    control.write_all(&reply).await.map_err(|e| e.to_string())?;
    let mut buffer = vec![0; 65535];
    let mut control_buffer = [0; 1];
    let mut endpoint = if requested_port == 0 {
        None
    } else {
        Some(std::net::SocketAddr::new(peer.ip(), requested_port))
    };
    loop {
        tokio::select! {
            _ = control.read(&mut control_buffer) => return Ok(()),
            _ = agent.closed.changed() => return Err("UDP agent channel closed".into()),
            packet = socket.recv_from(&mut buffer) => {
                let (length, source) = packet.map_err(|e| e.to_string())?;
                if source.ip() != peer.ip() || endpoint.map(|expected| expected != source).unwrap_or(false) { continue; }
                let payload_len = match udp_protocol::parse_packet(&buffer[..length]) { Ok(packet) => packet.payload.len(), Err(_) => continue };
                endpoint = Some(source);
                agent.send(buffer[..length].to_vec()).await?;
                counters.add_tx(payload_len as u64);
            }
            Some(packet) = agent.incoming.recv() => {
                if let Some(endpoint) = endpoint {
                    socket.send_to(&packet,endpoint).await.map_err(|e|e.to_string())?;
                    let packet = udp_protocol::parse_packet(&packet).map_err(|e|e.to_string())?;
                    counters.add_rx(packet.payload.len() as u64);
                }
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
