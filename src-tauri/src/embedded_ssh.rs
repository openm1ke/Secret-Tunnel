use russh::client;
use russh::keys::known_hosts::learn_known_hosts;
use russh::keys::{check_known_hosts, load_secret_key, ssh_key, PrivateKeyWithHashAlg};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, ToSocketAddrs};
use tokio::sync::Mutex;

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

#[derive(Debug)]
pub struct EmbeddedTunnelHandle {
    stop_tx: Sender<()>,
    join: Option<JoinHandle<()>>,
    counters: Arc<TrafficCounters>,
}

impl EmbeddedTunnelHandle {
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
}

impl client::Handler for SshClient {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        match self.strict_host_key_checking.as_str() {
            "no" => Ok(true),
            "accept-new" => match check_known_hosts(&self.host, self.port, server_public_key) {
                Ok(true) => Ok(true),
                Ok(false) => {
                    Ok(learn_known_hosts(&self.host, self.port, server_public_key).is_ok())
                }
                Err(_) => Ok(false),
            },
            _ => Ok(check_known_hosts(&self.host, self.port, server_public_key).unwrap_or(false)),
        }
    }
}

type SharedSession = Arc<Mutex<client::Handle<SshClient>>>;

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
    if lower.contains("key") && lower.contains("reject") || lower.contains("unknownkey") {
        return format!(
            "Embedded SSH rejected the server host key for {}:{}. Check ~/.ssh/known_hosts or use StrictHostKeyChecking=accept-new for the first connection.",
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

pub fn start(server: EmbeddedServer, proxy: EmbeddedProxy) -> Result<EmbeddedTunnelHandle, String> {
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
    let join = thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let _ = runtime.block_on(run_embedded_tunnel(
            listener,
            server,
            stop_rx,
            tunnel_counters,
        ));
    });

    Ok(EmbeddedTunnelHandle {
        stop_tx,
        join: Some(join),
        counters,
    })
}

async fn run_embedded_tunnel(
    listener: TcpListener,
    server: EmbeddedServer,
    stop_rx: mpsc::Receiver<()>,
    counters: Arc<TrafficCounters>,
) -> Result<(), String> {
    let listener =
        tokio::net::TcpListener::from_std(listener).map_err(|error| error.to_string())?;
    let session = connect_ssh(&server, (server.host.as_str(), server.port)).await?;
    let session = Arc::new(Mutex::new(session));

    loop {
        if stop_rx.try_recv().is_ok() {
            disconnect_ssh(&session).await;
            return Ok(());
        }

        tokio::select! {
            accepted = listener.accept() => {
                let (stream, originator_addr) = accepted.map_err(|error| error.to_string())?;
                let session = Arc::clone(&session);
                let counters = Arc::clone(&counters);
                tokio::spawn(async move {
                    let _ = handle_socks_client(stream, originator_addr, session, counters).await;
                });
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }
}

async fn connect_ssh<A: ToSocketAddrs>(
    server: &EmbeddedServer,
    addrs: A,
) -> Result<client::Handle<SshClient>, String> {
    let key_pair = load_secret_key(&server.key_path, None)
        .map_err(|error| friendly_key_error(&server.key_path, error))?;

    let config = Arc::new(client::Config {
        nodelay: true,
        inactivity_timeout: Some(Duration::from_secs(30)),
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        ..Default::default()
    });
    let mut session = client::connect(
        config,
        addrs,
        SshClient {
            host: server.host.clone(),
            port: server.port,
            strict_host_key_checking: server.strict_host_key_checking.clone(),
        },
    )
    .await
    .map_err(|error| friendly_connect_error(server, error))?;

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
    let session = session.lock().await;
    let _ = session
        .disconnect(russh::Disconnect::ByApplication, "", "English")
        .await;
}

async fn handle_socks_client(
    mut stream: TcpStream,
    originator_addr: std::net::SocketAddr,
    session: SharedSession,
    counters: Arc<TrafficCounters>,
) -> Result<(), String> {
    let request = match read_socks5_request(&mut stream).await {
        Ok(request) => request,
        Err(error) => {
            let _ = stream.shutdown().await;
            return Err(error);
        }
    };

    let channel = {
        let session = session.lock().await;
        session
            .channel_open_direct_tcpip(
                request.host.clone(),
                request.port.into(),
                originator_addr.ip().to_string(),
                originator_addr.port().into(),
            )
            .await
            .map_err(|error| error.to_string())?
    };

    write_socks5_reply(&mut stream, 0x00).await?;
    let mut channel_stream = channel.into_stream();
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
    let mut client_closed = false;
    let mut remote_closed = false;
    let mut client_buf = vec![0_u8; 32 * 1024];
    let mut remote_buf = vec![0_u8; 32 * 1024];

    while !client_closed || !remote_closed {
        tokio::select! {
            read = client.read(&mut client_buf), if !client_closed => {
                match read.map_err(|error| error.to_string())? {
                    0 => {
                        client_closed = true;
                        remote.shutdown().await.map_err(|error| error.to_string())?;
                    }
                    n => {
                        remote.write_all(&client_buf[..n]).await.map_err(|error| error.to_string())?;
                        counters.add_tx(n as u64);
                    }
                }
            }
            read = remote.read(&mut remote_buf), if !remote_closed => {
                match read.map_err(|error| error.to_string())? {
                    0 => {
                        remote_closed = true;
                        client.shutdown().await.map_err(|error| error.to_string())?;
                    }
                    n => {
                        client.write_all(&remote_buf[..n]).await.map_err(|error| error.to_string())?;
                        counters.add_rx(n as u64);
                    }
                }
            }
        }
    }

    Ok(())
}

#[derive(Clone, Debug)]
struct SocksRequest {
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
    if head[1] != 0x01 {
        write_socks5_reply(stream, 0x07).await?;
        return Err("Only SOCKS CONNECT is supported".to_string());
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
        host,
        port: u16::from_be_bytes(port),
    })
}

async fn write_socks5_reply(stream: &mut TcpStream, status: u8) -> Result<(), String> {
    stream
        .write_all(&[0x05, status, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await
        .map_err(|error| error.to_string())
}
